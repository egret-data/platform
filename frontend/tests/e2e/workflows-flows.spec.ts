import { test, expect, type Locator, type Page } from "@playwright/test";
import { firstWorkspace, loginLocalAdmin } from "./helpers";

/**
 * W7a (workflow UI, flows) end to end: author, validate, save, run, version
 * and compare a flow in the browser, and move the editor's panels by pointer
 * and by keyboard (spec `docs/en/specs/2026-10-10-workflow-w7a-flows-design.md`
 * §11, FE-TEST-2).
 *
 * Opt-in (`EGRET_E2E_WORKFLOWS=1`). It needs a backend with the workflow
 * worker on (`EGRET_WORKFLOW__WORKER_ENABLED=true`), or the execution never
 * leaves CREATED, and the break-glass local admin (it signs in once:
 * sign-in is rate limited to 5 per 5 minutes). The flow uses only core
 * tasks: default deployment settings allow no container image, so no script
 * type is offered.
 *
 * HOW TO RUN IT, on a stack of its own and never the shared dev stack (no
 * Postgres :5432, no shared bucket, no kind cluster). From the egret repo root:
 *
 * ```bash
 * # 1. A database of its own on the branch's Postgres (:5441), a private RustFS with one bucket.
 * docker exec egret-w7-postgres psql -U egret -d postgres -c "CREATE DATABASE egret_w7a_e2e"
 * docker run -d --name w7a-t8-rustfs -p 9447:9000 \
 *   -e RUSTFS_ACCESS_KEY=egret -e RUSTFS_SECRET_KEY=egretsecret rustfs/rustfs:1.0.1
 * curl --aws-sigv4 "aws:amz:us-east-1:s3" --user egret:egretsecret -X PUT http://localhost:9447/warehouse
 *
 * # 2. The backend on :8097, pointed only at those two.
 * export DATABASE_URL=postgres://egret:egret@localhost:5441/egret_w7a_e2e
 * export EGRET_DATABASE__URL=$DATABASE_URL EGRET_SERVER__PORT=8097 EGRET_SERVER__HOST=127.0.0.1
 * export EGRET_OIDC__ENABLED=false EGRET_WORKFLOW__WORKER_ENABLED=true
 * export EGRET_STORAGE__ENDPOINT=http://localhost:9447 EGRET_STORAGE__BUCKET=warehouse
 * export EGRET_STORAGE__ACCESS_KEY=egret EGRET_STORAGE__SECRET_KEY=egretsecret
 * # Only while iterating, and only on this private backend, NEVER on a shared stack: sign-in
 * # is limited to 5 attempts per 5 minutes, and every run signs in once.
 * # export EGRET_RATE_LIMIT__LOGIN_MAX_ATTEMPTS=200
 * cargo build -p egret-bin
 * ./target/debug/egret migrate && ./target/debug/egret init   # admin@egret.local / Admin12345!
 * ./target/debug/egret serve
 *
 * # 3. The frontend on :5197, proxying /api to :8097 (the committed config next to this spec;
 * #    EGRET_E2E_VITE_PORT and EGRET_E2E_API override the two).
 * cd frontend && npx vite --config tests/e2e/vite.e2e.config.mts
 *
 * # 4. The spec.
 * cd frontend && EGRET_E2E_WORKFLOWS=1 EGRET_E2E_BASE_URL=http://localhost:5197 \
 *   npx playwright test workflows-flows
 *
 * # 5. Afterwards: stop both servers, `docker rm -f w7a-t8-rustfs`, and drop the database by its name:
 * docker exec egret-w7-postgres psql -U egret -d postgres -c "DROP DATABASE egret_w7a_e2e"
 * ```
 */
const ENABLED = process.env.EGRET_E2E_WORKFLOWS === "1";

const RUN = Date.now().toString(36);
const FLOW_ID = `w7a-e2e-${RUN}`;
const NAMESPACE = "e2e.w7a";

const SOURCE = `id: ${FLOW_ID}
namespace: ${NAMESPACE}
inputs:
  - id: greeting
    type: STRING
    defaults: hello
tasks:
  - id: say
    type: io.kestra.plugin.core.log.Log
    message: "{{ inputs.greeting }} from W7a"
`;

/** The flow editor's Monaco (its `ariaLabel` is "Flow source"). */
const codeEditor = (page: Page): Locator => page.locator(".monaco-editor").filter({ has: page.getByRole("textbox", { name: "Flow source" }) });

/** The editor frame's tab strips: one per panel. */
const panels = (page: Page): Locator => page.getByRole("tablist", { name: /^Panel \d+$/ });

/**
 * Monaco reads the platform from the user agent, and the "Desktop Chrome"
 * device is Windows: its shortcuts are Control, whatever the host is.
 */
const MOD = "Control";

/**
 * Paste `text` at the cursor, as the browser would: one `paste` event, which
 * Monaco inserts as it is. Typing it (or `insertText`) would auto-indent each
 * new line under the last.
 */
async function paste(page: Page, text: string): Promise<void> {
  await page.evaluate((t) => {
    const target = document.activeElement ?? document.body;
    const data = new DataTransfer();
    data.setData("text/plain", t);
    target.dispatchEvent(new ClipboardEvent("paste", { clipboardData: data, bubbles: true, cancelable: true }));
  }, text);
}

/** Replace the whole text of the flow editor. */
async function replaceSource(page: Page, text: string): Promise<void> {
  await codeEditor(page).locator(".view-lines").click();
  await page.keyboard.press(`${MOD}+A`);
  await paste(page, text);
}

/** Put the cursor at the end of `lineNumber` (1-based) through Monaco's own "go to line". */
async function cursorToLineEnd(page: Page, lineNumber: number): Promise<void> {
  await codeEditor(page).locator(".view-lines").click();
  await page.keyboard.press(`${MOD}+Home`);
  for (let i = 1; i < lineNumber; i++) await page.keyboard.press("ArrowDown");
  await page.keyboard.press("End");
}

/** The line Monaco shows as the cursor's. */
const activeLine = (page: Page): Locator => codeEditor(page).locator(".line-numbers.active-line-number");

/** The frame's tab named `name` (Code, Docs, Problems; Problems carries its error count). */
const tab = (page: Page, name: string | RegExp): Locator => page.getByRole("tab", { name });

/**
 * Drag `source` by pointer from `fromX` px inside its left edge and drop it at
 * (x, y), in steps so dnd-kit sees a real movement.
 */
async function dragTo(page: Page, source: Locator, x: number, y: number, fromX?: number): Promise<void> {
  const box = (await source.boundingBox())!;
  const startX = box.x + (fromX ?? box.width / 2);
  await page.mouse.move(startX, box.y + box.height / 2);
  await page.mouse.down();
  await page.mouse.move(startX + 12, box.y + box.height / 2 + 4, { steps: 3 });
  await page.mouse.move(x, y, { steps: 12 });
  await page.mouse.up();
}

test.describe("W7a flows: author, run, version, panels", () => {
  test.skip(!ENABLED, "opt-in: set EGRET_E2E_WORKFLOWS=1 with a local stack up (see the comment above)");
  test.setTimeout(180_000);

  test("create → completion → error marked and listed → Save → Run → SUCCESS → Create version → diff; panels by pointer and keyboard", async ({ page }) => {
    await loginLocalAdmin(page);
    const ws = (await firstWorkspace(page.request)).slug;

    // ── 1. Create a flow ────────────────────────────────────────────────
    await page.goto(`/explore/${ws}/workflows/flows/new`);
    await expect(page.getByRole("heading", { name: "New flow" })).toBeVisible();
    await expect(codeEditor(page)).toBeVisible({ timeout: 30_000 });
    // The template validates clean first: the schema and the worker are up.
    await expect(tab(page, /^Problems$/)).toBeVisible();
    await replaceSource(page, SOURCE);

    // ── 2. `type: ` completes, and lists the core Log task ──────────────
    // A second task on a new line after the last; `type: ` then Ctrl+Space.
    await cursorToLineEnd(page, 10);
    await page.keyboard.press("Enter");
    // The new line is indented under the list item; drop to the list's column.
    await page.keyboard.press("Shift+Home");
    await paste(page, "  - id: second\n    type: ");
    // A Ctrl+Space in the same tick as the paste is cancelled by Monaco's paste
    // handling, so it is asked again until the list is up.
    const suggest = codeEditor(page).locator(".suggest-widget.visible");
    await expect(async () => {
      await page.keyboard.press("Control+Space");
      await expect(suggest).toBeVisible({ timeout: 2_000 });
    }).toPass({ timeout: 20_000 });
    // The list is long (every type and alias); narrowing it brings the core Log task in view.
    await page.keyboard.type("core.log.Log");
    await expect(suggest.getByText("io.kestra.plugin.core.log.Log", { exact: true }).first()).toBeVisible();
    await page.keyboard.press("Escape");

    // ── 3. An error is marked in the editor and listed in Problems ──────
    // A second task with the first one's id: a server error (`duplicate_task_id`, path
    // `tasks.say`). An unknown type would do less: egret only warns about one.
    await replaceSource(page, `${SOURCE}  - id: say\n    type: io.kestra.plugin.core.log.Log\n    message: again\n`);
    await expect(codeEditor(page).locator(".squiggly-error").first()).toBeVisible({ timeout: 15_000 });
    // Named by its aria-label, exactly (a hidden count once read "Problems , 1 error" in Chromium).
    const problemsTab = tab(page, /^Problems, 1 error$/);
    await expect(problemsTab).toBeVisible({ timeout: 15_000 });
    await problemsTab.click();
    const problem = page.getByRole("list", { name: "Problems" }).getByRole("button", { name: /declared more than once/ });
    await expect(problem).toBeVisible();
    // Move the cursor away first, so the click is what puts it on the error.
    await cursorToLineEnd(page, 1);
    await expect(activeLine(page)).toHaveText("1");
    await problem.click();
    // A repeated id maps to the first task that holds it (spec §15): `- id: say` on line 8.
    await expect(activeLine(page)).toHaveText("8");
    await expect(page.getByRole("textbox", { name: "Flow source" })).toBeFocused();

    // Back to a valid flow: the source without the second task.
    await replaceSource(page, SOURCE);
    await expect(tab(page, /^Problems$/)).toBeVisible({ timeout: 15_000 });

    // ── 4. Save: the first Save creates the flow and opens its editor ───
    const save = page.getByRole("button", { name: "Save", exact: true });
    await expect(save).toBeEnabled();
    await save.click();
    await page.waitForURL(/\/workflows\/flows\/[0-9a-f-]{36}\/edit$/, { timeout: 15_000 });
    const flowUrl = new URL(page.url()).pathname;
    await expect(page.getByRole("heading", { name: FLOW_ID })).toBeVisible();
    await expect(page.getByTestId("flow-latest-version")).toHaveText("Version —");

    // ── 5. Run the saved draft with one STRING input; it reaches SUCCESS ─
    const run = page.getByRole("button", { name: "Run", exact: true });
    await expect(run).toBeEnabled({ timeout: 15_000 });
    await run.click();
    const runDialog = page.getByRole("dialog", { name: `Run ${NAMESPACE}.${FLOW_ID}` });
    await expect(runDialog).toBeVisible();
    const greeting = runDialog.getByLabel(/greeting/);
    await expect(greeting).toHaveValue("hello");
    await greeting.fill("hi");
    await runDialog.getByRole("button", { name: "Run", exact: true }).click();
    await page.waitForURL(/\/workflows\/executions\/[^/]+$/, { timeout: 15_000 });
    const summary = page.getByTestId("execution-summary");
    await expect(summary).toContainText(`${NAMESPACE}.${FLOW_ID}`);
    await expect(summary.locator('[data-state="SUCCESS"]')).toBeVisible({ timeout: 60_000 });
    await expect(page.getByRole("cell", { name: "say" })).toBeVisible();

    // ── 6. Create version with a note ───────────────────────────────────
    await page.goto(flowUrl);
    await expect(codeEditor(page)).toBeVisible({ timeout: 30_000 });
    const createVersion = page.getByRole("button", { name: "Create version", exact: true });
    await expect(createVersion).toBeEnabled({ timeout: 15_000 });
    await createVersion.click();
    const versionDialog = page.getByRole("dialog", { name: `Create a version of ${NAMESPACE}.${FLOW_ID}` });
    await versionDialog.getByLabel("Note (optional)").fill("first e2e version");
    await versionDialog.getByRole("button", { name: "Create version" }).click();
    await expect(versionDialog).toBeHidden();
    await expect(page.getByTestId("flow-latest-version")).toHaveText("Version v1");

    // A saved change after v1: the pill compares content.
    await cursorToLineEnd(page, 10);
    await page.keyboard.press("Backspace"); // the closing quote
    await paste(page, ', changed"');
    await expect(page.getByText("Unsaved changes", { exact: true })).toBeVisible();
    await save.click();
    await expect(page.getByText("Changes since v1")).toBeVisible({ timeout: 15_000 });
    await expect(page.getByText("Unsaved changes", { exact: true })).toBeHidden();

    // ── 7. Pointer: drag Docs to the Code panel's right edge → three panels ─
    await page.getByRole("tab", { name: "Edit" }).click();
    await expect(panels(page)).toHaveCount(2);
    const codePanelBody = page.getByRole("tabpanel", { name: "Code", exact: true });
    const body = (await codePanelBody.boundingBox())!;
    // From 2px inside the Docs tab, beside the resize gutter: the gutter's hit area
    // once reached 3px into the tab and turned this drag into a resize.
    await dragTo(page, tab(page, "Docs"), body.x + body.width - 20, body.y + body.height / 2, 2);
    await expect(panels(page)).toHaveCount(3);
    await expect(panels(page).nth(1).getByRole("tab")).toHaveText(["Docs"]);
    await page.reload();
    await expect(codeEditor(page)).toBeVisible({ timeout: 30_000 });
    await expect(panels(page)).toHaveCount(3);

    // ── 8. Keyboard: "Move to panel…" then "Split right" from the tab menu ──
    await tab(page, "Docs").focus();
    await page.keyboard.press("Tab");
    await expect(page.getByRole("button", { name: "Tab actions: Docs" })).toBeFocused();
    await page.keyboard.press("Enter");
    const moveTo = page.getByRole("menuitem", { name: "Move to panel…" });
    await expect(moveTo).toBeFocused();
    await page.keyboard.press("ArrowRight");
    // The submenu lists the other panels in order; it opens on the first.
    await expect(page.getByRole("menuitem", { name: "Panel 1 (Code)" })).toBeFocused();
    await page.keyboard.press("ArrowDown");
    const toPanel3 = page.getByRole("menuitem", { name: "Panel 3 (Problems)" });
    await expect(toPanel3).toBeFocused();
    await page.keyboard.press("Enter");
    await expect(panels(page)).toHaveCount(2);
    await expect(panels(page).nth(1).getByRole("tab")).toHaveText(["Problems", "Docs"]);
    await expect(tab(page, "Docs")).toBeFocused();
    await page.keyboard.press("Tab");
    await page.keyboard.press("Enter");
    await expect(moveTo).toBeFocused();
    // One arrow at a time: Radix moves focus a frame later, and a second arrow before
    // that lands moves from the old item.
    await page.keyboard.press("ArrowDown");
    await expect(page.getByRole("menuitem", { name: "Split left" })).toBeFocused();
    await page.keyboard.press("ArrowDown");
    await expect(page.getByRole("menuitem", { name: "Split right" })).toBeFocused();
    await page.keyboard.press("Enter");
    await expect(panels(page)).toHaveCount(3);
    await expect(panels(page).nth(2).getByRole("tab")).toHaveText(["Docs"]);

    // ── 9. Revisions: compare v1 with the saved draft ───────────────────
    await page.getByRole("tab", { name: "Revisions" }).click();
    await expect(page.getByRole("heading", { name: "v1" })).toBeVisible();
    await expect(page.getByRole("cell", { name: "first e2e version" })).toBeVisible();
    await page.getByLabel("Compare with").click();
    await page.getByRole("option", { name: "Current draft" }).click();
    await expect(page).toHaveURL(/compare=draft/);
    const diff = page.locator(".monaco-diff-editor");
    await expect(diff).toBeVisible({ timeout: 15_000 });
    // v1 on the left, the draft (with ", changed") on the right.
    await expect(diff.locator(".editor.original")).toContainText("from W7a\"");
    await expect(diff.locator(".editor.modified")).toContainText("from W7a, changed\"");
    await expect(diff.locator(".char-insert, .line-insert").first()).toBeVisible();
  });
});
