#!/usr/bin/env node
// DEMO ONLY. Browser check of "Add engine" on the demo stack (Docker provisioner): as the seeded
// admin, add a single-node Trino engine on the seeded warehouse, wait until it runs, assign it to
// the workspace, run `SELECT 1` and a query on the warehouse catalog in SQL Lab through it, find
// the query in the engine's monitor, delete the engine, and check that its containers are gone.
// Prints "OK <step>" per step; exits 1 naming the failing step, with a screenshot.
//   DEMO_PORT=18080 node deploy/demo/verify-engine.mjs
// Env: BASE, DEMO_PORT, WS (default "default"), ENGINE (default "demo-e2e"), CATALOG (the seeded
// warehouse's registry catalog, default "demo_warehouse"), DEMO_PROJECT (default "egret-demo"),
// RUNNING_TIMEOUT_S (default 300: the first use pulls trinodb/trino), DELEGATION ("default" leaves
// per-user delegation at Auto, which resolves to JWT here; "impersonate" picks Impersonate and turns
// JWT authentication on, which assigning a workspace needs), STRICT_MONITOR.
// The monitor step: egret's query monitor does not list queries on these engines today (known
// product issues, see the demo guide's Known behaviour: no egret-monitor user on a JWT-delegation
// engine, `GET /v1/node` answers 404 on Trino 483, and the monitor user has no Ranger grant to see
// other users' queries). The step passes when the query is listed; otherwise it requires the monitor's
// "Engine not reachable" banner and prints KNOWN ISSUE, or fails when STRICT_MONITOR=1.
// KEEP_ENGINE=1 stops after the monitor step and leaves the engine running (for manual checks).
// STATE (session cookies) and SHOT_DIR default to the system temp dir, never the repo.
import { execFileSync } from "node:child_process";
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
const STATE = process.env.STATE ?? path.join(os.tmpdir(), "egret-demo-verify-engine-state.json");
const SHOT_DIR = process.env.SHOT_DIR ?? os.tmpdir();
const EMAIL = process.env.EGRET_ADMIN_EMAIL ?? "admin@egret.local";
const PASSWORD = process.env.EGRET_ADMIN_PASSWORD ?? "Admin12345!";
const WS = process.env.WS ?? "default";
const ENGINE = process.env.ENGINE ?? "demo-e2e";
const CATALOG = process.env.CATALOG ?? "demo_warehouse";
const PROJECT = process.env.DEMO_PROJECT ?? "egret-demo";
const RUNNING_MS = Number(process.env.RUNNING_TIMEOUT_S ?? 300) * 1000;
const DELEGATION = process.env.DELEGATION ?? "default";
if (!["default", "impersonate"].includes(DELEGATION)) {
  console.error(`DELEGATION must be default or impersonate, not ${DELEGATION}`);
  process.exit(2);
}
// A marker that makes this run's query findable in the monitor.
const MARK = `demo_e2e_${Date.now().toString(36)}`;

fs.mkdirSync(SHOT_DIR, { recursive: true });
const browser = await chromium.launch();
let page;
let current = "launch";

async function step(name, fn) {
  current = name;
  const t0 = Date.now();
  await fn();
  console.log(`OK ${name} (${((Date.now() - t0) / 1000).toFixed(1)}s)`);
}

/** Containers of this engine in this demo project (docker CLI on the host, read only). */
function engineContainers() {
  const out = execFileSync(
    "docker",
    ["ps", "-a", "--filter", `label=egret.engine=${ENGINE}`, "--filter", `label=egret.project=${PROJECT}`, "--format", "{{.Names}}"],
    { encoding: "utf8" },
  );
  return out.split("\n").filter(Boolean);
}

async function shot(name) {
  await page.screenshot({ path: path.join(SHOT_DIR, `verify-engine-${name}.png`), fullPage: true }).catch(() => {});
}

/** The engine-detail header (holds the type and phase badges and the actions). */
const header = () => page.locator("div.shrink-0", { has: page.getByRole("button", { name: "More engine actions" }) });

/** Open the engine page from the Control Plane list; null when no such engine is listed. */
async function openEngine() {
  await page.goto(`${BASE}/control-plane/engines`);
  await page.getByRole("button", { name: "Add engine" }).waitFor({ timeout: 30_000 });
  const link = page.getByRole("link", { name: ENGINE, exact: true });
  // The list loads after the header; give it a moment before deciding the engine is absent.
  try { await link.waitFor({ timeout: 5_000 }); } catch { return null; }
  await link.click();
  await page.waitForURL(/\/control-plane\/engines\/[0-9a-f-]{36}/, { timeout: 15_000 });
  await header().waitFor({ timeout: 30_000 });
  return page.url().split("?")[0];
}

/** Delete the open engine through its page actions, then wait until egret lists it no more. */
async function deleteFromUi() {
  await page.getByRole("button", { name: "More engine actions" }).click();
  await page.getByRole("menuitem", { name: "Delete engine" }).click();
  const dialog = page.getByRole("alertdialog");
  await dialog.getByPlaceholder(ENGINE, { exact: true }).fill(ENGINE);
  await dialog.getByRole("button", { name: "Delete", exact: true }).click();
  await page.waitForURL((u) => u.pathname === "/control-plane/engines", { timeout: 120_000 });
  // Reload: the list can show the cached row after the delete (the delete does not refresh it).
  const link = page.getByRole("link", { name: ENGINE, exact: true });
  for (let i = 0; i < 12; i++) {
    await page.reload();
    await page.getByRole("button", { name: "Add engine" }).waitFor({ timeout: 30_000 });
    await page.waitForLoadState("networkidle", { timeout: 10_000 }).catch(() => {});
    if (!(await link.isVisible())) return;
  }
  throw new Error(`${ENGINE} is still listed after the delete`);
}

/** Run `sql` in SQL Lab against the open tab's database; `expect` must then match a result cell. */
async function runSql(sql, expect = null) {
  const editor = page.locator(".monaco-editor").first();
  await editor.click();
  await page.keyboard.press("ControlOrMeta+a");
  await page.keyboard.press("Delete");
  await page.keyboard.insertText(sql);
  // Wait for the submit, so the previous query's result (cleared on Run) cannot satisfy the check.
  const submitted = page.waitForResponse(
    (r) => r.request().method() === "POST" && new URL(r.url()).pathname === `/api/ws/${WS}/queries`,
    { timeout: 60_000 },
  );
  await page.getByRole("button", { name: "Run", exact: true }).click();
  const res = await submitted;
  if (!res.ok()) throw new Error(`submit ${res.status()}: ${sql}: ${(await res.text()).slice(0, 300)}`);
  const failed = page.getByText("Query failed", { exact: true });
  const done = page.getByText(/^Done in /).first();
  await failed.or(done).first().waitFor({ timeout: 120_000 });
  if (await failed.isVisible()) {
    const msg = await page.locator(".whitespace-pre-wrap.text-destructive").first().innerText().catch(() => "");
    throw new Error(`query failed: ${sql}: ${msg.split("\n")[0]}`);
  }
  if (expect) await page.locator("table td", { hasText: expect }).first().waitFor({ timeout: 30_000 });
}

try {
  await step("login", async () => {
    if (fs.existsSync(STATE)) {
      // A cached session can outlive `demo-reset`; keep it only while the server still accepts it.
      const probe = await browser.newContext({ storageState: STATE });
      const res = await probe.request.get(`${BASE}/api/control-plane/engines`);
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

  await step("leftover", async () => {
    // An earlier failed run can leave the engine behind; remove it the same way the check does.
    if (await openEngine()) {
      console.log(`  an engine named ${ENGINE} exists from an earlier run; deleting it first`);
      await deleteFromUi();
    }
    for (let i = 0; i < 30 && engineContainers().length > 0; i++) await page.waitForTimeout(2_000);
    const left = engineContainers();
    if (left.length) throw new Error(`containers of ${ENGINE} still exist: ${left.join(", ")}`);
  });

  let engineUrl;
  await step("add engine", async () => {
    await page.goto(`${BASE}/control-plane/engines`);
    await page.getByRole("button", { name: "Add engine" }).click();
    await page.waitForURL(/\/control-plane\/engines\/new/, { timeout: 15_000 });
    await page.getByRole("heading", { name: "New engine" }).waitFor();
    // Engine type: Trino is the default of the Trino tab; check rather than assume.
    const type = await page.locator("#ec-type").innerText();
    if (type.trim() !== "Trino") throw new Error(`engine type is ${type.trim()}, expected Trino`);
    await page.locator("#ec-name").fill(ENGINE);
    await page.getByRole("radio", { name: `Make ${CATALOG} primary` }).click({ timeout: 15_000 });
    // Single node: no workers (the coordinator then schedules work on itself).
    await page.getByRole("button", { name: "Worker", exact: true }).click();
    await page.locator("#ced-worker-replicas").fill("0");
    if (DELEGATION === "impersonate") {
      await page.getByRole("button", { name: "Security & Exposure", exact: true }).click();
      await page.locator("#deleg-impersonate").click();
      const jwt = page.locator("#ced-jwt-enabled");
      if ((await jwt.getAttribute("aria-checked")) !== "true") await jwt.click();
    }
    await shot("add-engine-form");
    await page.getByRole("button", { name: "Create engine" }).click();
    await page.waitForURL(/\/control-plane\/engines\/[0-9a-f-]{36}/, { timeout: 60_000 });
    engineUrl = page.url().split("?")[0];
    await header().waitFor({ timeout: 30_000 });
  });

  await step("running", async () => {
    const t0 = Date.now();
    await header().getByText("running", { exact: true }).waitFor({ timeout: RUNNING_MS });
    console.log(`  status running after ${((Date.now() - t0) / 1000).toFixed(1)}s; containers: ${engineContainers().join(", ")}`);
    await shot("running");
  });

  await step("assign workspace", async () => {
    await page.getByRole("tab", { name: "Workspaces" }).click();
    const add = page.getByRole("button", { name: "Add workspace" });
    await add.waitFor();
    if (await add.isDisabled()) throw new Error("Add workspace is disabled: the engine has no JWT authentication");
    await add.click();
    const dialog = page.getByRole("dialog");
    await dialog.getByRole("button", { name: new RegExp(`\\b${WS}\\b`) }).first().click();
    await page.getByText("Workspace assigned").waitFor({ timeout: 30_000 });
  });

  await step("sql lab", async () => {
    await page.goto(`${BASE}/explore/${WS}/sql-lab`);
    // A fresh tab, so an earlier run's tab state (database, SQL) does not leak in.
    await page.getByRole("button", { name: "New tab" }).click();
    await page.getByRole("combobox").filter({ hasText: /Select a database|\(/ }).first().click();
    await page.getByRole("option", { name: new RegExp(`^${ENGINE} \\(trino\\)`) }).click({ timeout: 30_000 });
    await runSql(`SELECT 1 AS ${MARK}`, /^\s*1\s*$/);
    await runSql(`SHOW SCHEMAS FROM ${CATALOG}`, "information_schema");
    // Write and read the warehouse: Iceberg REST catalog (egret) plus object storage (RustFS).
    const schema = `${CATALOG}.demo_e2e`;
    const table = `${schema}.${MARK}`;
    await runSql(`CREATE SCHEMA IF NOT EXISTS ${schema}`);
    await runSql(`CREATE TABLE ${table} AS SELECT * FROM (VALUES 1, 2, 3) AS t(x)`, /^\s*3\s*$/);
    await runSql(`SELECT sum(x) AS total FROM ${table}`, /^\s*6\s*$/);
    await shot(`sql-lab-${DELEGATION}`);
    await runSql(`DROP TABLE ${table}`);
  });

  await step("monitor", async () => {
    await page.goto(`${engineUrl}?tab=monitor`);
    await page.getByRole("tab", { name: "Monitor" }).click();
    const listed = page.getByRole("cell", { name: new RegExp(MARK) }).first();
    const banner = page.getByText("Engine not reachable — is a coordinator running?");
    if (process.env.STRICT_MONITOR === "1") {
      await listed.waitFor({ timeout: 60_000 });
    } else {
      // The banner comes from the cluster summary; give the list its own chance to show the query.
      await listed.or(banner).first().waitFor({ timeout: 60_000 });
      await listed.waitFor({ timeout: 15_000 }).catch(() => {});
      if (!(await listed.isVisible())) {
        console.log("  KNOWN ISSUE: the monitor does not list the query (see the header of this script); STRICT_MONITOR=1 makes this fail");
      }
    }
    await shot(`monitor-${DELEGATION}`);
  });

  if (process.env.KEEP_ENGINE === "1") {
    console.log(`ALL OK up to the monitor; KEEP_ENGINE=1 leaves ${ENGINE} running: ${engineUrl}`);
    await browser.close();
    process.exit(0);
  }

  await step("delete engine", async () => {
    await page.goto(engineUrl);
    await header().waitFor({ timeout: 30_000 });
    await deleteFromUi();
  });

  await step("containers removed", async () => {
    for (let i = 0; i < 30 && engineContainers().length > 0; i++) await page.waitForTimeout(2_000);
    const left = engineContainers();
    if (left.length) throw new Error(`containers of ${ENGINE} still exist: ${left.join(", ")}`);
  });

  console.log(`ALL OK (screenshots in ${SHOT_DIR})`);
  await browser.close();
} catch (e) {
  const shotPath = path.join(SHOT_DIR, `verify-engine-FAIL-${current.replace(/\s+/g, "-")}.png`);
  if (page) await page.screenshot({ path: shotPath, fullPage: true }).catch(() => {});
  console.error(`FAIL ${current}: ${e.message.split("\n")[0]}`);
  console.error(`screenshot: ${shotPath}`);
  await browser.close();
  process.exit(1);
}
