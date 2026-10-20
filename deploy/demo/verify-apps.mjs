#!/usr/bin/env node
// DEMO ONLY. Browser check of the Data Apps lifecycle on the demo stack: create an app in the UI,
// open its editor (marimo, served from the apps origin inside an iframe), run a cell, publish,
// open the published app as a viewer. Prints "OK <step>" per step; exits 1 with a screenshot.
//   DEMO_PORT=18080 node deploy/demo/verify-apps.mjs
// Env: BASE, DEMO_PORT, DEMO_APPS_URL (or DEMO_APPS_HOST; set DEMO_APPS_URL when BASE is not on localtest.me).
// STATE (session cookies) and SHOT_DIR default to the system temp dir, never the repo.
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { createRequire } from "node:module";

// Playwright is resolved from this script's folders, then the current directory, then PLAYWRIGHT_MODULE_DIR
// (a folder that holds node_modules/playwright).
let chromium;
const lookups = [import.meta.url, path.join(process.cwd(), "noop.js")];
if (process.env.PLAYWRIGHT_MODULE_DIR) lookups.push(path.join(process.env.PLAYWRIGHT_MODULE_DIR, "noop.js"));
for (const from of lookups) {
  try {
    ({ chromium } = createRequire(from)("playwright"));
    break;
  } catch {}
}
if (!chromium) {
  console.error("playwright not found: run `npm i playwright` (and `npx playwright install chromium`) or set PLAYWRIGHT_MODULE_DIR");
  process.exit(2);
}

const BASE = process.env.BASE ?? `http://egret.localtest.me:${process.env.DEMO_PORT ?? "8080"}`;
const baseUrl = new URL(BASE);
// Apps origin: DEMO_APPS_URL, else DEMO_APPS_HOST (default apps.localtest.me) on BASE's port.
const APPS = new URL(process.env.DEMO_APPS_URL ?? `${baseUrl.protocol}//${process.env.DEMO_APPS_HOST ?? "apps.localtest.me"}${baseUrl.port ? `:${baseUrl.port}` : ""}`).host;
const STATE = process.env.STATE ?? path.join(os.tmpdir(), "egret-demo-verify-apps-state.json");
const SHOT_DIR = process.env.SHOT_DIR ?? os.tmpdir();
const EMAIL = process.env.EGRET_ADMIN_EMAIL ?? "admin@egret.local";
const PASSWORD = process.env.EGRET_ADMIN_PASSWORD ?? "Admin12345!";
const WS = process.env.WS ?? "default";
const NAME = `Demo app ${new Date().toISOString().replace(/[-:T]/g, "").slice(0, 14)}`;
const POD_START_MS = 180_000; // a cold marimo start inside the container

const browser = await chromium.launch();
let page;
let current = "launch";

async function step(name, fn) {
  current = name;
  const t0 = Date.now();
  await fn();
  console.log(`OK ${name} (${((Date.now() - t0) / 1000).toFixed(1)}s)`);
}

/** The marimo frame: the iframe whose document is on the apps origin. */
async function appsFrame(iframeSelector) {
  const handle = await page.locator(iframeSelector).elementHandle({ timeout: 60_000 });
  const frame = await handle.contentFrame();
  await frame.waitForURL((u) => u.host === APPS, { timeout: POD_START_MS });
  return frame;
}

try {
  await step("login", async () => {
    if (fs.existsSync(STATE)) {
      // A cached session can outlive `demo-reset`; keep it only while the server still accepts it.
      const probe = await browser.newContext({ storageState: STATE });
      const res = await probe.request.get(`${BASE}/api/ws/${WS}/apps/capabilities`);
      await probe.close();
      if (res.ok()) return;
      fs.rmSync(STATE, { force: true });
    }
    const ctx = await browser.newContext();
    const p = (page = await ctx.newPage());
    await p.goto(`${BASE}/login`);
    // With SSO on, the local form hides behind the break-glass link (once the auth config loads).
    const breakGlass = p.getByRole("button", { name: "Admin sign-in (break-glass)" });
    await breakGlass.or(p.getByPlaceholder("Email")).first().waitFor({ timeout: 30_000 });
    const email = p.getByPlaceholder("Email");
    // The page can re-render (auth config) and drop the form, or swallow the click; retry the pair.
    for (let i = 0; ; i++) {
      if (!(await email.isVisible()) && (await breakGlass.isVisible())) await breakGlass.click();
      try { await email.fill(EMAIL, { timeout: 4_000 }); break; } catch (e) { if (i >= 6) throw e; }
    }
    await p.getByPlaceholder("Password").fill(PASSWORD);
    await p.getByRole("button", { name: "Sign in", exact: true }).click();
    await p.waitForURL((u) => !u.pathname.startsWith("/login"), { timeout: 30_000 });
    await ctx.storageState({ path: STATE });
    await ctx.close();
  });

  const ctx = await browser.newContext({ storageState: STATE, viewport: { width: 1400, height: 900 } });
  page = await ctx.newPage();
  page.on("console", (m) => { if (m.type() === "error") console.log(`  console: ${m.text()}`); });

  let appUrl;
  await step("create", async () => {
    await page.goto(`${BASE}/explore/${WS}/apps`);
    await page.getByRole("button", { name: "+ App" }).click();
    await page.getByLabel("Name").fill(NAME);
    await page.getByRole("button", { name: "Create", exact: true }).click();
    await page.waitForURL(/\/apps\/[0-9a-f-]+\/edit$/, { timeout: 30_000 });
    appUrl = page.url().replace(/\/edit$/, "");
    await page.getByRole("link", { name: "Back to apps" }).click();
    await page.getByRole("link", { name: NAME }).waitFor({ timeout: 15_000 });
    await page.goto(`${appUrl}/edit`); // a fresh ticket: the first editor load was abandoned
  });

  await step("editor", async () => {
    const t0 = Date.now();
    const frame = await appsFrame("iframe[title$=' editor']");
    await frame.getByText("New data app").first().waitFor({ timeout: POD_START_MS });
    console.log(`  time to first editor load: ${((Date.now() - t0) / 1000).toFixed(1)}s`);
    const draftRev = async () =>
      (await (await page.request.get(`${BASE}/api/ws/${WS}${new URL(appUrl).pathname.replace(/^\/explore\/[^/]+/, "")}`)).json()).draft_revision;
    const before = await draftRev();
    // Add a Python cell at the end, type 1 + 1 and run it.
    await frame.getByRole("button", { name: /^Python$/ }).last().click();
    const editor = frame.locator(".cm-editor .cm-content").last();
    await editor.click();
    await editor.pressSequentially("1 + 1");
    await editor.press("Control+Enter");
    await frame.locator(".output-area").filter({ hasText: /^\s*2\s*$/ }).first().waitFor({ timeout: 60_000 });
    // Workaround for a product issue: Publish right after Ctrl+S can return 409, because the editor
    // header refreshes the draft revision only every 5 s. So wait until the server has a newer
    // revision AND the button shows it.
    await editor.press("Control+s");
    let rev = before;
    for (let i = 0; i < 30 && rev <= before; i++) { await page.waitForTimeout(1000); rev = await draftRev(); }
    if (rev <= before) throw new Error(`draft not saved (revision still ${before})`);
    await page.getByRole("button", { name: `Publish draft rev ${rev}`, exact: true }).waitFor({ timeout: 15_000 });
  });

  await step("publish", async () => {
    await page.getByRole("button", { name: /^Publish draft rev \d+$/ }).click();
    await page.getByText("Published version 1").waitFor({ timeout: 30_000 });
    await page.getByText("Published v1", { exact: true }).waitFor({ timeout: 15_000 });
    // Workaround for a product issue: the Versions dialog is not refreshed after a publish
    // (usePublishApp in frontend/src/api/apps.ts does not invalidate the versions query), so reload.
    await page.reload();
    await page.getByRole("button", { name: "Versions" }).click();
    await page.getByRole("dialog").getByText("v1", { exact: true }).waitFor({ timeout: 15_000 });
    await page.keyboard.press("Escape");
  });

  await step("view", async () => {
    await page.goto(appUrl);
    const frame = await appsFrame(`iframe[title='${NAME}']`);
    await frame.getByText("New data app").first().waitFor({ timeout: POD_START_MS });
    await frame.getByText(/^\s*2\s*$/).first().waitFor({ timeout: 60_000 });
  });

  console.log(`ALL OK ${appUrl}`);
  await browser.close();
} catch (e) {
  const shot = path.join(SHOT_DIR, `verify-apps-${current}.png`);
  if (page) await page.screenshot({ path: shot, fullPage: true }).catch(() => {});
  console.error(`FAIL ${current}: ${e.message.split("\n")[0]}`);
  console.error(`screenshot: ${shot}`);
  await browser.close();
  process.exit(1);
}
