import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { devices, test, expect, type Browser, type Locator, type Page, type Request, type TestInfo } from "@playwright/test";
import { csrf, firstWorkspace, loginLocalAdmin, postJson } from "./helpers";

/**
 * W7d (workflow UI, namespaces) end to end, spec
 * `docs/en/specs/2026-10-14-workflow-w7d-namespaces-design.md` §6:
 *
 * - a flow in `company.team` and a flow in `company.other` that calls it through a `Subflow`;
 * - on the namespace page: a file created, edited, kept with its unsaved dot across a visit to
 *   the KV tab, and saved with Ctrl+S (its POST carries `if_revision`), renamed, moved by a real drag and drop, a folder imported through the
 *   `webkitdirectory` picker, the namespace exported as a zip; the tree and the editor measured, in
 *   light and dark mode (the editor's background asserted dark in dark mode), and an editor that
 *   was hidden lays itself out when shown again;
 * - KV: a JSON key and a DATE key created, a NUMBER with an exponent read back with every digit,
 *   and the keys inherited from `company`;
 * - the flow editor: Code beside TWO file tabs, side by side; Ctrl+S in each file saves only that
 *   file (one POST with `if_revision`, no flow PUT), and in Code only the flow; measured at the
 *   default width; in dark mode the file editors and Code are dark, and stay dark while and after a
 *   revision opens in the Revisions drawer (Monaco's theme is global);
 * - a save from a second session makes the next one answer 409 `file_changed`: the stale dialog,
 *   and Overwrite resends without `if_revision`;
 * - a flow saved into another namespace while a file has unsaved edits asks first, and "Save files
 *   first" writes into the OLD namespace;
 * - Dependencies draws the `subflow` edge between `company.other` and `company.team`.
 *
 * Opt-in (`EGRET_E2E_WORKFLOWS=1`). It signs in twice (the second session of the 409), within the
 * sign-in limit of 5 per 5 minutes, and uses only core tasks.
 *
 * HOW TO RUN IT, on a stack of its own and never the shared dev stack (no Postgres :5432, no
 * shared bucket, no kind cluster). From the egret repo root:
 *
 * ```bash
 * # 1. A database of its own on the branch's Postgres (:5445), a private RustFS with one bucket
 * #    (namespace files are stored there). The branch's Postgres, once, if it does not exist yet:
 * docker run -d --name egret-w7d-postgres -p 5445:5432 -e POSTGRES_USER=egret -e POSTGRES_PASSWORD=egret postgres:16
 * docker exec egret-w7d-postgres psql -U egret -d postgres -c "CREATE DATABASE egret_w7d_live"
 * docker run -d --name w7d-rustfs -p 9477:9000 \
 *   -e RUSTFS_ACCESS_KEY=egret -e RUSTFS_SECRET_KEY=egretsecret rustfs/rustfs:1.0.1
 * curl --aws-sigv4 "aws:amz:us-east-1:s3" --user egret:egretsecret -X PUT http://localhost:9477/warehouse
 *
 * # 2. The backend on :8217, pointed only at those two.
 * export DATABASE_URL=postgres://egret:egret@localhost:5445/egret_w7d_live
 * export EGRET_DATABASE__URL=$DATABASE_URL EGRET_SERVER__PORT=8217 EGRET_SERVER__HOST=127.0.0.1
 * export EGRET_OIDC__ENABLED=false EGRET_WORKFLOW__WORKER_ENABLED=true
 * export EGRET_STORAGE__ENDPOINT=http://localhost:9477 EGRET_STORAGE__BUCKET=warehouse
 * export EGRET_STORAGE__ACCESS_KEY=egret EGRET_STORAGE__SECRET_KEY=egretsecret
 * export EGRET_ADMIN_PASSWORD='Admin12345!'
 * # Only while iterating, and only on this private backend: sign-in is limited to 5 per 5 minutes.
 * # export EGRET_RATE_LIMIT__LOGIN_MAX_ATTEMPTS=200
 * cargo build -p egret-bin
 * ./target/debug/egret migrate && ./target/debug/egret init   # admin@egret.local
 * ./target/debug/egret serve
 *
 * # 3. The frontend on :5227, proxying /api to :8217 (the committed config next to this spec).
 * cd frontend && EGRET_E2E_VITE_PORT=5227 EGRET_E2E_API=http://localhost:8217 \
 *   npx vite --config tests/e2e/vite.e2e.config.mts
 *
 * # 4. The spec. `EGRET_E2E_SHOTS=<dir>` also saves a screenshot of each measured state there.
 * cd frontend && EGRET_E2E_WORKFLOWS=1 EGRET_E2E_BASE_URL=http://localhost:5227 \
 *   npx playwright test workflows-namespaces
 *
 * # 5. Afterwards: stop both servers, `docker rm -f -v w7d-rustfs`, and drop the database by its name:
 * docker exec egret-w7d-postgres psql -U egret -d postgres -c "DROP DATABASE egret_w7d_live"
 * ```
 *
 * Each run uses its own flow ids, file paths and keys, so it can be repeated on the same database.
 */
const ENABLED = process.env.EGRET_E2E_WORKFLOWS === "1";
const SHOTS = process.env.EGRET_E2E_SHOTS;

const RUN = Date.now().toString(36);
const TEAM = "company.team";
const OTHER = "company.other";
const PARENT = "company";
const MOVED = `company.moved${RUN}`;
const CHILD_ID = `child-${RUN}`;
const CALLER_ID = `caller-${RUN}`;
const MOVER_ID = `mover-${RUN}`;
/** Every file of this run lives under its own folder, so a re-run on the same database starts clean. */
const DIR = `/run-${RUN}`;
const A = `${DIR}/scripts/a.py`;
const C = `${DIR}/scripts/c.py`;

const CHILD = `id: ${CHILD_ID}
namespace: ${TEAM}
tasks:
  - id: hello
    type: io.kestra.plugin.core.log.Log
    message: from the child
`;

const CALLER = `id: ${CALLER_ID}
namespace: ${OTHER}
tasks:
  - id: call
    type: io.kestra.plugin.core.flow.Subflow
    namespace: ${TEAM}
    flowId: ${CHILD_ID}
    wait: true
`;

const MOVER = `id: ${MOVER_ID}
namespace: ${TEAM}
tasks:
  - id: hello
    type: io.kestra.plugin.core.log.Log
    message: moves later
`;

/** Playwright's "Desktop Chrome" reports Windows, so Monaco's CtrlCmd is Control here. */
const MOD = "Control";

/** Monaco reads a paste event's text; `keyboard.type` would auto-indent every line. */
async function paste(page: Page, text: string): Promise<void> {
  await page.evaluate((t) => {
    const target = document.activeElement ?? document.body;
    const data = new DataTransfer();
    data.setData("text/plain", t);
    target.dispatchEvent(new ClipboardEvent("paste", { clipboardData: data, bubbles: true, cancelable: true }));
  }, text);
}

/** A Monaco editor, by the `ariaLabel` its textarea carries. */
const monaco = (page: Page, label: string): Locator => page.locator(".monaco-editor").filter({ has: page.getByRole("textbox", { name: label, exact: true }) });
const fileEditor = (page: Page, name: string): Locator => monaco(page, `Contents of ${name}`);
const codeEditor = (page: Page): Locator => monaco(page, "Flow source");
/** The same, found also while a modal drawer hides the page from the accessibility tree (`aria-hidden`). */
const monacoBehindModal = (page: Page, label: string): Locator =>
  page.locator(".monaco-editor").filter({ has: page.getByRole("textbox", { name: label, exact: true, includeHidden: true }) });

/**
 * Focus a Monaco editor through its input. A click on its lines can land on a neighbour's
 * overview ruler when panels sit side by side.
 */
async function focusEditor(editor: Locator): Promise<void> {
  await editor.getByRole("textbox").focus({ timeout: 30_000 });
  await expect(editor.getByRole("textbox")).toBeFocused();
}

/** Put the cursor at the end of an editor's text and paste there. */
async function appendTo(page: Page, editor: Locator, text: string): Promise<void> {
  await focusEditor(editor);
  await page.keyboard.press(`${MOD}+End`);
  await paste(page, text);
}

/** Replace an editor's whole text. */
async function replaceIn(page: Page, editor: Locator, text: string): Promise<void> {
  await focusEditor(editor);
  await page.keyboard.press(`${MOD}+A`);
  await paste(page, text);
}

/** Light or dark, through the app's own preference (`ThemeProvider`, `egret-theme`), then a reload. */
async function setTheme(page: Page, theme: "light" | "dark"): Promise<void> {
  await page.evaluate((t) => localStorage.setItem("egret-theme", t), theme);
  await page.reload();
  await expect(page.locator("html")).toHaveClass(theme === "dark" ? /\bdark\b/ : /^(?!.*\bdark\b)/);
}

const frame = (): Promise<void> => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(() => r())));

/** The writes this page sent: namespace-file POSTs and flow PUTs, in order. */
interface Write {
  kind: "file" | "flow";
  namespace: string;
  path: string | null;
  ifRevision: string | null;
  url: string;
}

function recordWrites(page: Page): Write[] {
  const writes: Write[] = [];
  page.on("request", (r: Request) => {
    const url = new URL(r.url());
    const file = /\/workflows\/namespaces\/([^/]+)\/files$/.exec(url.pathname);
    if (r.method() === "POST" && file) {
      writes.push({ kind: "file", namespace: decodeURIComponent(file[1]), path: url.searchParams.get("path"), ifRevision: url.searchParams.get("if_revision"), url: r.url() });
    }
    if (r.method() === "PUT" && /\/workflows\/flows\/[^/]+$/.test(url.pathname)) {
      writes.push({ kind: "flow", namespace: "", path: null, ifRevision: null, url: r.url() });
    }
  });
  return writes;
}

/** Every measurement, printed as it is taken (for the report). */
function report(key: string, value: unknown): void {
  console.log(`measure [${key}]: ${JSON.stringify(value)}`);
}

async function shot(page: Page, key: string): Promise<void> {
  if (SHOTS) await page.screenshot({ path: `${SHOTS}/w7d-${key.replace(/\W+/g, "-")}.png` });
}

interface Box {
  width: number;
  height: number;
}
const size = async (l: Locator): Promise<Box> => {
  const b = (await l.boundingBox())!;
  return { width: Math.round(b.width), height: Math.round(b.height) };
};

/**
 * A Monaco editor fills the box it is in, and draws its text: its width and height are the
 * container's (within a pixel or two), and its first line has a box. A Monaco that was laid out
 * while hidden is 0×0, or keeps the size it had elsewhere.
 */
async function measureMonaco(editor: Locator): Promise<{ editor: Box; container: Box; lineWidth: number; background: string }> {
  return editor.evaluate((el) => {
    const r = el.getBoundingClientRect();
    const host = el.parentElement!.getBoundingClientRect();
    const line = el.querySelector(".view-line")?.getBoundingClientRect();
    return {
      editor: { width: Math.round(r.width), height: Math.round(r.height) },
      container: { width: Math.round(host.width), height: Math.round(host.height) },
      lineWidth: Math.round(line?.width ?? 0),
      background: getComputedStyle(el.querySelector(".monaco-editor-background") ?? el).backgroundColor,
    };
  });
}

function expectFills(m: { editor: Box; container: Box; lineWidth: number }, what: string, minHeight: number): void {
  report(what, m);
  expect(Math.abs(m.editor.width - m.container.width), `${what}: editor width vs its box`).toBeLessThanOrEqual(2);
  expect(Math.abs(m.editor.height - m.container.height), `${what}: editor height vs its box`).toBeLessThanOrEqual(2);
  expect(m.editor.height, `${what}: editor height`).toBeGreaterThanOrEqual(minHeight);
  expect(m.lineWidth, `${what}: first line drawn`).toBeGreaterThan(0);
}

/** A CSS `rgb()`/`rgba()` colour's relative brightness, 0 (black) to 1 (white). */
function brightness(css: string): number {
  const [r, g, b] = (css.match(/\d+(?:\.\d+)?/g) ?? []).map(Number);
  return (0.2126 * r + 0.7152 * g + 0.0722 * b) / 255;
}

/**
 * An editor's background follows the app's mode. Monaco's theme is global, so one editor that
 * names no theme turns every editor light in dark mode (W7d's final review): measured, not assumed.
 */
function expectTheme(m: { background: string }, theme: "light" | "dark", what: string): void {
  report(`${what} background`, m.background);
  const b = brightness(m.background);
  if (theme === "dark") expect(b, `${what}: a dark background, not ${m.background}`).toBeLessThan(0.3);
  else expect(b, `${what}: a light background, not ${m.background}`).toBeGreaterThan(0.7);
}

/** Expand every folder above `file` in a file tree (rows carry their stored path as `title`), then open it. */
async function openInTree(tree: Locator, file: string): Promise<void> {
  const parts = file.split("/").filter(Boolean);
  for (let i = 1; i < parts.length; i++) {
    const row = tree.locator(`[role="treeitem"][title="/${parts.slice(0, i).join("/")}/"]`);
    await expect(row).toBeVisible({ timeout: 30_000 });
    if ((await row.getAttribute("aria-expanded")) !== "true") await row.click();
    await expect(row).toHaveAttribute("aria-expanded", "true");
  }
  await tree.locator(`[role="treeitem"][title="${file}"]`).click();
}

/** One more signed-in session (the second writer of the 409). */
async function secondSession(browser: Browser, use: TestInfo["project"]["use"]): Promise<Page> {
  // The project's device too: its Windows user agent is what makes Monaco's CtrlCmd Control (see MOD).
  const { defaultBrowserType: _browser, ...device } = devices["Desktop Chrome"];
  const ctx = await browser.newContext({ ...device, baseURL: use.baseURL });
  const page = await ctx.newPage();
  await loginLocalAdmin(page);
  return page;
}

test.describe("W7d namespaces: list, files, KV, file tabs, 409, namespace change, dependencies", () => {
  test.skip(!ENABLED, "opt-in: set EGRET_E2E_WORKFLOWS=1 with a private local stack up (see the comment above)");
  test.setTimeout(600_000);

  // Vite optimises a dependency the first time a module needs it (Monaco's workers, xyflow) and then
  // reloads every open page, which would land mid-test. Load those modules once on a throwaway page
  // first, until a load passes without a reload.
  test.beforeAll(async ({ browser }, testInfo) => {
    test.setTimeout(120_000);
    const page = await browser.newPage({ baseURL: testInfo.project.use.baseURL });
    try {
      for (let i = 0; i < 4; i++) {
        await page.goto("/login");
        await page.evaluate(() => Object.assign(window, { egretWarm: true }));
        await page
          .evaluate(async (paths) => {
            await Promise.all(paths.map((p) => import(/* @vite-ignore */ p)));
            const env = (self as unknown as { MonacoEnvironment?: { getWorker: (id: string, label: string) => Worker } }).MonacoEnvironment;
            for (const label of ["yaml", "json", "editorWorkerService"]) env?.getWorker("", label);
          }, [
            "/src/lib/monaco.ts",
            "/src/components/workflows/editor/registerEditorTabs.tsx",
            "/src/components/workflows/editor/files/FileTab.tsx",
            "/src/components/workflows/editor/files/FilesTab.tsx",
            "/src/components/workflows/namespaces/NamespaceFilesTab.tsx",
            "/src/components/workflows/namespaces/NamespaceKvTab.tsx",
            "/src/components/workflows/namespaces/NamespaceDependenciesTab.tsx",
            "/src/routes/workflows/NamespaceDetail.tsx",
            "/src/routes/workflows/NamespacesList.tsx",
          ])
          .catch(() => undefined);
        await page.waitForTimeout(3_000);
        const kept = await page.evaluate(() => "egretWarm" in window).catch(() => false);
        if (kept) break;
      }
    } finally {
      await page.close();
    }
  });

  test("namespaces → files → KV → file tabs side by side → 409 → namespace change → dependencies", async ({ page, context, browser }, testInfo) => {
    page.on("console", (m) => { if (m.type() === "error") console.log(`[console ${m.type()}] ${m.text().slice(0, 300)}`); });
    page.on("pageerror", (e) => console.log(`[pageerror] ${e.message}`));
    const writes = recordWrites(page);
    await page.setViewportSize({ width: 1600, height: 900 });
    await loginLocalAdmin(page);
    await setTheme(page, "light");
    const ws = (await firstWorkspace(page.request)).slug;
    const api = `/api/ws/${ws}/workflows`;
    const h = await csrf(context);
    const base = `/explore/${ws}/workflows`;
    const nsApi = (ns: string) => `${api}/namespaces/${encodeURIComponent(ns)}`;

    // ── 1. The flows, through the API (W7a drives the editor), and a key in the parent ─────
    const child = await postJson(page.request, `${api}/flows`, h, { source: CHILD });
    const caller = await postJson(page.request, `${api}/flows`, h, { source: CALLER });
    const mover = await postJson(page.request, `${api}/flows`, h, { source: MOVER });
    for (const f of [child, caller, mover]) expect(f.validation.errors, JSON.stringify(f.validation)).toEqual([]);
    const parentKey = `inherited_${RUN}`;
    const putParent = await page.request.put(`${nsApi(PARENT)}/kv/${parentKey}`, { headers: { ...h, "Content-Type": "text/plain" }, data: '"from the parent"' });
    expect(putParent.ok(), `PUT parent key → ${putParent.status()} ${await putParent.text()}`).toBeTruthy();

    // ── 2. The list: company and its children, as a tree ──────────────────────────────────
    await test.step("namespace list", async () => {
      await page.goto(`${base}/namespaces`);
      const table = page.getByTestId("namespaces-table");
      await expect(table.getByRole("link", { name: TEAM, exact: true })).toBeVisible({ timeout: 30_000 });
      await expect(table.getByRole("link", { name: OTHER, exact: true })).toBeVisible();
      await expect(table.getByRole("link", { name: PARENT, exact: true })).toBeVisible();
      await table.getByRole("link", { name: TEAM, exact: true }).click();
      await expect(page).toHaveURL(new RegExp(`/namespaces/${TEAM.replace(".", "\\.")}/overview$`));
      for (const t of ["Overview", "Flows", "Executions", "Dependencies", "Files", "KV"]) {
        await expect(page.getByRole("tab", { name: t, exact: true })).toBeVisible();
      }
    });

    // ── 3. Files on the namespace page ───────────────────────────────────────────────────
    const filesUrl = `${base}/namespaces/${TEAM}/files`;
    const tree = page.getByRole("tree", { name: `Files of ${TEAM}` });
    /** A tree row by its stored path (a folder's ends with `/`). */
    const item = (p: string) => tree.locator(`[role="treeitem"][title="${p}"]`);
    /** Expand a folder row unless it is open. */
    const expand = async (row: Locator) => {
      if ((await row.getAttribute("aria-expanded")) !== "true") await row.click();
      await expect(row).toHaveAttribute("aria-expanded", "true");
    };
    /** Where "New file" and imports write: the folder the page says, under the toolbar. */
    const targetDir = async () => (await page.getByText("New files and imports go into").locator(".font-mono").textContent())!;
    const createFile = async (p: string) => {
      const dir = await targetDir();
      expect(p.startsWith(dir), `${p} is under the target folder ${dir}`).toBe(true);
      await page.getByRole("button", { name: "New file" }).first().click();
      const dialog = page.getByRole("dialog", { name: "New file" });
      await dialog.getByLabel("Name").fill(p.slice(dir.length));
      await dialog.getByRole("button", { name: "Create file" }).click();
      await expect(dialog).toBeHidden();
    };

    await test.step("files: create, edit, Ctrl+S with if_revision, rename, drag and drop, folder import, export", async () => {
      await page.goto(filesUrl);
      await expect(page.getByRole("button", { name: "New file" }).first()).toBeEnabled({ timeout: 30_000 });
      // Every create is a write that must not exist yet: `if_revision=0`.
      const before = writes.length;
      await createFile(A);
      await expect(fileEditor(page, "a.py")).toBeVisible({ timeout: 30_000 });
      const created = writes.slice(before).filter((w) => w.kind === "file");
      expect(created.map((w) => [w.path, w.ifRevision])).toEqual([[A, "0"]]);
      await createFile(C);
      await expect(fileEditor(page, "c.py")).toBeVisible({ timeout: 30_000 });

      // Edit a.py, Ctrl+S: one POST, with the revision it was read at (1).
      await page.getByRole("tab", { name: "a.py" }).click();
      await appendTo(page, fileEditor(page, "a.py"), "print('saved on the page')\n");
      await expect(page.getByRole("tablist", { name: "Open files" }).getByTestId("unsaved-dot")).toHaveCount(1);

      // Another tab of the namespace and back: the unsaved text and its dot are still there.
      await page.getByRole("tab", { name: "KV", exact: true }).click();
      await expect(page).toHaveURL(/\/kv$/);
      await expect(fileEditor(page, "a.py")).toBeHidden();
      await page.getByRole("tab", { name: "Files", exact: true }).click();
      await expect(page).toHaveURL(/\/files$/);
      await expect(fileEditor(page, "a.py")).toBeVisible();
      await expect(fileEditor(page, "a.py").locator(".view-lines")).toContainText("print('saved on the page')");
      await expect(page.getByRole("tablist", { name: "Open files" }).getByTestId("unsaved-dot")).toHaveCount(1);
      // Ctrl+S belongs to the editor with focus.
      await focusEditor(fileEditor(page, "a.py"));

      const beforeSave = writes.length;
      const saved = page.waitForResponse((r) => r.request().method() === "POST" && r.url().includes("/files?") && r.url().includes("a.py"));
      await page.keyboard.press(`${MOD}+s`);
      expect((await saved).status()).toBe(200);
      expect(writes.slice(beforeSave).map((w) => [w.kind, w.path, w.ifRevision])).toEqual([["file", A, "1"]]);
      await expect(page.getByRole("tablist", { name: "Open files" }).getByTestId("unsaved-dot")).toHaveCount(0);

      // An editor that was hidden lays itself out when it is shown again.
      await page.getByRole("tab", { name: "c.py" }).click();
      await page.setViewportSize({ width: 1400, height: 860 });
      await page.getByRole("tab", { name: "a.py" }).click();
      await page.evaluate(frame);
      expectFills(await measureMonaco(fileEditor(page, "a.py")), "namespace page a.py after hidden + resize", 300);

      // Heights: the tree and the editor fill the tab, light then dark.
      for (const theme of ["light", "dark"] as const) {
        if (theme === "dark") {
          await setTheme(page, "dark");
          await expect(item(`${DIR}/`)).toBeVisible({ timeout: 30_000 });
          // The open tabs are remembered per namespace.
          await expect(fileEditor(page, "a.py")).toBeVisible({ timeout: 30_000 });
        }
        const m = { tree: await size(tree), editor: await measureMonaco(fileEditor(page, "a.py")), viewport: page.viewportSize() };
        report(`namespace files tab ${theme}`, m);
        expect(m.tree.height, `${theme}: tree height`).toBeGreaterThanOrEqual(300);
        expectFills(m.editor, `namespace files editor ${theme}`, 300);
        expectTheme(m.editor, theme, `namespace files editor ${theme}`);
        await shot(page, `namespace-files-${theme}`);
      }
      await setTheme(page, "light");
      await page.setViewportSize({ width: 1600, height: 900 });

      // Expand the run's folder so its files are rows.
      const runDir = item(`${DIR}/`);
      const scripts = item(`${DIR}/scripts/`);
      await expect(runDir).toBeVisible({ timeout: 30_000 });
      await expand(runDir);
      await expand(scripts);
      await expect(item(A)).toBeVisible();

      // Rename: a temporary file, so a.py and c.py keep their paths for the editor below.
      await item(A).click(); // a file selected: new files go into its folder
      await createFile(`${DIR}/scripts/old_name.sql`);
      await item(`${DIR}/scripts/old_name.sql`).getByRole("button", { name: "Actions for old_name.sql" }).click();
      await page.getByRole("menuitem", { name: "Rename" }).click();
      const rename = page.getByRole("dialog", { name: "Rename old_name.sql" });
      await rename.getByLabel("New name").fill("renamed.sql");
      await rename.getByRole("button", { name: "Rename" }).click();
      await expect(item(`${DIR}/scripts/renamed.sql`)).toBeVisible();
      await expect(item(`${DIR}/scripts/old_name.sql`)).toHaveCount(0);

      // A real drag and drop: renamed.sql onto a new folder `archive` in the run's folder.
      await runDir.click(); // selects the run's folder (and folds it): the new folder goes inside it
      expect(await targetDir()).toBe(`${DIR}/`);
      await page.getByRole("button", { name: "New folder" }).click();
      const folder = page.getByRole("dialog", { name: "New folder" });
      await folder.getByLabel("Name").fill("archive");
      await folder.getByRole("button", { name: "Create folder" }).click();
      await expect(folder).toBeHidden();
      await expand(runDir);
      await expand(scripts);
      const archive = item(`${DIR}/archive/`);
      await expect(archive).toBeVisible();
      await item(`${DIR}/scripts/renamed.sql`).dragTo(archive);
      await expect(item(`${DIR}/archive/renamed.sql`).or(archive)).toBeVisible();
      await expect.poll(async () => (await page.request.get(`${nsApi(TEAM)}/files/stats?path=${encodeURIComponent(`${DIR}/archive/renamed.sql`)}`)).status(), { timeout: 15_000 }).toBe(200);
      expect((await page.request.get(`${nsApi(TEAM)}/files/stats?path=${encodeURIComponent(`${DIR}/scripts/renamed.sql`)}`)).status()).toBe(404);
      await expect(item(`${DIR}/scripts/renamed.sql`)).toHaveCount(0);

      // A folder import through the `webkitdirectory` picker: every file with its path below the folder.
      const local = fs.mkdtempSync(path.join(os.tmpdir(), "w7d-import-"));
      const imported = path.join(local, "imported");
      fs.mkdirSync(path.join(imported, "sub"), { recursive: true });
      fs.writeFileSync(path.join(imported, "top.txt"), "top\n");
      fs.writeFileSync(path.join(imported, "sub", "deep.py"), "print('deep')\n");
      try {
        await runDir.click();
        expect(await targetDir()).toBe(`${DIR}/`);
        const picker = page.getByLabel("Choose a folder to import");
        await expect(picker).toHaveAttribute("webkitdirectory", "");
        await picker.setInputFiles(imported);
        await expect.poll(async () => (await page.request.get(`${nsApi(TEAM)}/files/stats?path=${encodeURIComponent(`${DIR}/imported/sub/deep.py`)}`)).status(), { timeout: 15_000 }).toBe(200);
        expect((await page.request.get(`${nsApi(TEAM)}/files/stats?path=${encodeURIComponent(`${DIR}/imported/top.txt`)}`)).status()).toBe(200);
        await expand(runDir);
        await expect(item(`${DIR}/imported/`)).toBeVisible();
      } finally {
        fs.rmSync(local, { recursive: true, force: true });
      }

      // Export: the namespace as a zip.
      const download = page.waitForEvent("download");
      await page.getByRole("link", { name: "Export as zip" }).click();
      const zip = await download;
      expect(zip.suggestedFilename()).toBe(`${TEAM}_files.zip`);
      const bytes = fs.readFileSync((await zip.path())!);
      expect(bytes.subarray(0, 2).toString("latin1"), "a zip starts with PK").toBe("PK");
      expect(bytes.includes(Buffer.from(`run-${RUN}/scripts/a.py`)), "the zip names a.py").toBe(true);
    });

    // ── 4. KV ────────────────────────────────────────────────────────────────────────────
    await test.step("KV: JSON and DATE keys, an exponent NUMBER keeps its digits, inherited keys", async () => {
      await page.goto(`${base}/namespaces/${TEAM}/kv`);
      const addKey = async (key: string, type: string, fill: () => Promise<void>) => {
        // The empty state carries its own "Add a key" too.
        await page.getByRole("button", { name: "Add a key" }).first().click();
        const drawer = page.getByRole("dialog", { name: "Add a key" });
        await drawer.getByLabel("Key", { exact: true }).fill(key);
        await drawer.getByLabel("Type", { exact: true }).click();
        await page.getByRole("option", { name: type, exact: true }).click();
        await fill();
        await drawer.getByRole("button", { name: "Save", exact: true }).click();
        await expect(drawer).toBeHidden();
        await expect(page.locator(`tr[data-key="${key}"]`)).toBeVisible();
      };
      const jsonKey = `config_${RUN}`;
      await addKey(jsonKey, "JSON", async () => {
        await replaceIn(page, monaco(page, "Value"), '{"retries": 3, "big": 12345678901234567890123, "tags": ["a", "b"]}');
      });
      const dateKey = `start_${RUN}`;
      await addKey(dateKey, "DATE", async () => {
        await page.getByRole("dialog", { name: "Add a key" }).getByLabel("Value", { exact: true }).fill("2026-10-14");
      });
      const numKey = `exp_${RUN}`;
      await addKey(numKey, "NUMBER", async () => {
        await page.getByRole("dialog", { name: "Add a key" }).getByLabel("Value", { exact: true }).fill("1.2345678901234567890123e5");
      });

      // Read back through the API: the types, and every digit (a double keeps about 17).
      const raw = async (key: string) => (await page.request.get(`${nsApi(TEAM)}/kv/${key}`)).text();
      const json = await raw(jsonKey);
      expect(json).toContain('"type":"JSON"');
      expect(json).toContain("12345678901234567890123");
      expect(await raw(dateKey)).toMatch(/"type":"DATE".*"value":"2026-10-14"/);
      const num = await raw(numKey);
      expect(num).toContain('"type":"NUMBER"');
      expect(num).toContain("123456.78901234567890123");

      // And in the UI's value drawer.
      await page.locator(`tr[data-key="${numKey}"]`).getByRole("button", { name: `Actions for ${numKey}` }).click();
      await page.getByRole("menuitem", { name: "View value" }).click();
      await expect(page.getByLabel("Value", { exact: true })).toHaveText("123456.78901234567890123");
      await page.keyboard.press("Escape");

      // Inherited from the parent.
      await page.getByRole("button", { name: "Inherited keys" }).click();
      const inherited = page.getByRole("dialog", { name: "Inherited KV" });
      const row = inherited.getByRole("row").filter({ hasText: parentKey });
      await expect(row).toBeVisible();
      await expect(row.getByRole("cell", { name: PARENT, exact: true })).toBeVisible();
      await page.keyboard.press("Escape");
    });

    // ── 5. The flow editor: Code beside two file tabs; each Ctrl+S saves only its editor ───
    const tab = (name: string | RegExp) => page.getByRole("tab", { name });
    const panels = page.getByRole("tablist", { name: /^Panel \d+$/ });
    await test.step("editor: Code + two file tabs side by side, Ctrl+S per editor", async () => {
      await page.goto(`${base}/flows/${child.id}/edit`);
      await expect(codeEditor(page)).toBeVisible({ timeout: 30_000 });
      await page.getByRole("button", { name: "Reopen a tab in panel 2" }).click();
      await page.getByRole("menuitem", { name: "Files" }).click();
      const edTree = page.getByRole("tree", { name: `Files of ${TEAM}` });
      await expect(edTree.locator(`[role="treeitem"][title="${DIR}/"]`)).toBeVisible({ timeout: 30_000 });

      // The Files tab's tree fills its panel.
      const filesPanel = page.getByRole("tabpanel", { name: "Files", exact: true });
      const treeH = { panel: await size(filesPanel), tree: await size(edTree) };
      report("editor Files tab", treeH);
      expect(treeH.tree.height, "the tree in the editor's Files tab").toBeGreaterThanOrEqual(treeH.panel.height * 0.5);

      // a.py opens beside Code, at the default width; then hidden behind Code and shown again.
      await openInTree(edTree, A);
      await expect(fileEditor(page, "a.py")).toBeVisible({ timeout: 30_000 });
      await expect(panels.nth(0).getByRole("tab")).toHaveText([/Code/, /a\.py/]);
      expectFills(await measureMonaco(fileEditor(page, "a.py")), "editor a.py beside Code, default width", 300);
      await shot(page, "editor-file-tab-default-light");
      await tab(/^Code/).click();
      await expect(fileEditor(page, "a.py")).toBeHidden();
      await openInTree(edTree, C);
      await expect(fileEditor(page, "c.py")).toBeVisible({ timeout: 30_000 });
      await tab(/^a\.py/).click();
      await page.evaluate(frame);
      expectFills(await measureMonaco(fileEditor(page, "a.py")), "editor a.py shown again", 300);

      // Side by side: a.py, then c.py, each split to its own panel right of Code. Four panels:
      // a wider window, so each editor keeps a usable width.
      await page.setViewportSize({ width: 1920, height: 1000 });
      for (const name of ["a.py", "c.py"]) {
        await tab(new RegExp(`^${name.replace(".", "\\.")}`)).click();
        await page.getByRole("button", { name: `Tab actions: ${name}` }).click();
        await page.getByRole("menuitem", { name: "Split right" }).click();
      }
      await expect(fileEditor(page, "a.py")).toBeVisible();
      await expect(fileEditor(page, "c.py")).toBeVisible();
      await expect(codeEditor(page)).toBeVisible();
      const [codeBox, aBox, cBox] = [await codeEditor(page).boundingBox(), await fileEditor(page, "a.py").boundingBox(), await fileEditor(page, "c.py").boundingBox()];
      report("side by side", { code: codeBox, a: aBox, c: cBox });
      for (const b of [aBox!, cBox!]) expect(b.x, "a file editor sits right of Code").toBeGreaterThanOrEqual(codeBox!.x + codeBox!.width - 1);
      expect(Math.abs(aBox!.x - cBox!.x), "the two file editors are in different panels").toBeGreaterThan(50);
      for (const name of ["a.py", "c.py"]) expectFills(await measureMonaco(fileEditor(page, name)), `editor ${name} side by side`, 300);
      await shot(page, "editor-side-by-side-light");

      // Make all three dirty.
      await appendTo(page, fileEditor(page, "a.py"), "# a edited in the editor\n");
      await appendTo(page, fileEditor(page, "c.py"), "# c edited in the editor\n");
      await appendTo(page, codeEditor(page), "# the flow edited\n");
      await expect(page.getByTestId("file-tab-dirty")).toHaveCount(2);
      await expect(page.getByText("Unsaved changes", { exact: true })).toBeVisible();

      const pressSave = async (editor: Locator, expected: (w: Write[]) => void) => {
        await focusEditor(editor);
        const n = writes.length;
        await page.keyboard.press(`${MOD}+s`);
        // Let any request a wrong binding would send leave the page.
        await expect.poll(() => writes.length, { timeout: 10_000 }).toBeGreaterThan(n);
        await page.waitForTimeout(1_500);
        expected(writes.slice(n));
      };
      const revOf = async (p: string) => (await (await page.request.get(`${nsApi(TEAM)}/files/stats?path=${encodeURIComponent(p)}`)).json()).revision as number;
      const aRev = await revOf(A);
      const cRev = await revOf(C);
      await pressSave(fileEditor(page, "a.py"), (w) => expect(w.map((x) => [x.kind, x.path, x.ifRevision]), "Ctrl+S in a.py").toEqual([["file", A, String(aRev)]]));
      await expect(page.getByTestId("file-tab-dirty")).toHaveCount(1);
      expect(await revOf(C), "c.py untouched").toBe(cRev);
      await pressSave(fileEditor(page, "c.py"), (w) => expect(w.map((x) => [x.kind, x.path, x.ifRevision]), "Ctrl+S in c.py").toEqual([["file", C, String(cRev)]]));
      await expect(page.getByTestId("file-tab-dirty")).toHaveCount(0);
      await pressSave(codeEditor(page), (w) => expect(w.map((x) => x.kind), "Ctrl+S in Code").toEqual(["flow"]));
      await expect(page.getByText("Unsaved changes", { exact: true })).toBeHidden();
      expect(await (await page.request.get(`${nsApi(TEAM)}/files?path=${encodeURIComponent(A)}`)).text()).toContain("# a edited in the editor");

      // Dark mode: the file tab still fills its panel.
      await setTheme(page, "dark");
      await expect(fileEditor(page, "a.py")).toBeVisible({ timeout: 30_000 });
      await expect(codeEditor(page)).toBeVisible({ timeout: 30_000 });
      for (const name of ["a.py", "c.py"]) {
        const m = await measureMonaco(fileEditor(page, name));
        expectFills(m, `editor ${name} side by side dark`, 300);
        expectTheme(m, "dark", `editor ${name} side by side dark`);
      }
      expectTheme(await measureMonaco(codeEditor(page)), "dark", "Code beside the file tabs, dark");
      await shot(page, "editor-side-by-side-dark");

      // A revision opened in the drawer is dark too, and leaves every other editor dark.
      // The innermost tab panel holding a.py's editor (the panels nest; document order puts it last).
      await page.locator('[role="tabpanel"]').filter({ has: fileEditor(page, "a.py") }).last().getByRole("button", { name: "Revisions" }).click();
      const drawer = page.getByRole("dialog", { name: "Revisions of a.py" });
      await drawer.getByRole("button", { name: "View revision 1" }).click();
      const revisionEditor = monaco(page, "Revision 1 of a.py");
      await expect(revisionEditor).toBeVisible({ timeout: 30_000 });
      expectTheme(await measureMonaco(revisionEditor), "dark", "revision 1 in the drawer, dark");
      // The drawer is modal: the editors behind it are hidden from the accessibility tree, not the screen.
      for (const name of ["a.py", "c.py"]) {
        expectTheme(await measureMonaco(monacoBehindModal(page, `Contents of ${name}`)), "dark", `editor ${name} beside the open Revisions, dark`);
      }
      expectTheme(await measureMonaco(monacoBehindModal(page, "Flow source")), "dark", "Code beside the open Revisions, dark");
      await shot(page, "editor-revisions-dark");
      await page.keyboard.press("Escape");
      await expect(drawer).toBeHidden();
      for (const name of ["a.py", "c.py"]) expectTheme(await measureMonaco(fileEditor(page, name)), "dark", `editor ${name} after Revisions, dark`);
      expectTheme(await measureMonaco(codeEditor(page)), "dark", "Code after Revisions, dark");
      await setTheme(page, "light");
      await expect(fileEditor(page, "a.py")).toBeVisible({ timeout: 30_000 });
    });

    // ── 6. A second session saves a.py: this one's save answers 409, Overwrite resends ────
    await test.step("409 file_changed from a second session → stale dialog → Overwrite without if_revision", async () => {
      const other = await secondSession(browser, testInfo.project.use);
      try {
        await other.goto(filesUrl);
        const otherTree = other.getByRole("tree", { name: `Files of ${TEAM}` });
        // A fresh session has no remembered tabs: open a.py from the tree.
        await openInTree(otherTree, A);
        await expect(fileEditor(other, "a.py")).toBeVisible({ timeout: 30_000 });
        await appendTo(other, fileEditor(other, "a.py"), "# from the second session\n");
        // Ctrl+S saves only once the editor knows the text changed (its Save is enabled).
        await expect(other.getByRole("button", { name: "Save", exact: true })).toBeEnabled();
        const otherSaved = other.waitForResponse((r) => r.request().method() === "POST" && r.url().includes("/files?"), { timeout: 20_000 });
        await other.keyboard.press(`${MOD}+s`);
        expect((await otherSaved).status()).toBe(200);
      } finally {
        // Only the page: its context goes with the browser at the end of the run. Closing the
        // context here once hung until the test's timeout on a heavily loaded machine, and once
        // failed with Chromium's "Failed to find context", after the page had closed in 6 ms.
        await other.close();
      }

      // This session still holds the revision it read: its save is refused.
      await appendTo(page, fileEditor(page, "a.py"), "# mine wins\n");
      await expect(page.getByTestId("file-tab-dirty")).toHaveCount(1);
      await focusEditor(fileEditor(page, "a.py"));
      const n = writes.length;
      const refused = page.waitForResponse((r) => r.request().method() === "POST" && r.url().includes("/files?"), { timeout: 20_000 });
      await page.keyboard.press(`${MOD}+s`);
      const res = await refused;
      expect(res.status()).toBe(409);
      expect((await res.json()).code).toBe("file_changed");
      const dialog = page.getByRole("alertdialog", { name: "a.py changed since you opened it" });
      await expect(dialog).toBeVisible();
      await shot(page, "editor-409-dialog");
      const overwritten = page.waitForResponse((r) => r.request().method() === "POST" && r.url().includes("/files?"), { timeout: 20_000 });
      await dialog.getByRole("button", { name: "Overwrite" }).click();
      expect((await overwritten).status()).toBe(200);
      await expect(dialog).toBeHidden();
      const sent = writes.slice(n);
      expect(sent.map((w) => [w.kind, w.path]), "the refused save, then the overwrite").toEqual([["file", A], ["file", A]]);
      expect(sent[0].ifRevision, "the refused save carried if_revision").not.toBeNull();
      expect(sent[1].ifRevision, "Overwrite resends without if_revision").toBeNull();
      const text = await (await page.request.get(`${nsApi(TEAM)}/files?path=${encodeURIComponent(A)}`)).text();
      expect(text).toContain("# mine wins");
      expect(text).not.toContain("# from the second session");
    });

    // ── 7. A flow saved into another namespace while a file is dirty ──────────────────────
    await test.step("namespace change with a dirty file → three choices → Save files first writes into the old namespace", async () => {
      await page.goto(`${base}/flows/${mover.id}/edit`);
      await expect(codeEditor(page)).toBeVisible({ timeout: 30_000 });
      if (!(await page.getByRole("tree", { name: `Files of ${TEAM}` }).isVisible())) {
        await page.getByRole("button", { name: /^Reopen a tab in panel \d+$/ }).last().click();
        await page.getByRole("menuitem", { name: "Files" }).click();
      }
      const edTree = page.getByRole("tree", { name: `Files of ${TEAM}` });
      await openInTree(edTree, C);
      await expect(fileEditor(page, "c.py")).toBeVisible({ timeout: 30_000 });
      await appendTo(page, fileEditor(page, "c.py"), "# saved before the move\n");
      await expect(page.getByTestId("file-tab-dirty")).toHaveCount(1);

      // c.py opened in Code's panel and is the shown tab there: back to Code.
      await tab(/^Code/).click();
      await replaceIn(page, codeEditor(page), MOVER.replace(`namespace: ${TEAM}`, `namespace: ${MOVED}`));
      // Ctrl+S saves only once the page knows the flow changed.
      await expect(page.getByText("Unsaved changes", { exact: true })).toBeVisible();
      await focusEditor(codeEditor(page));
      const n = writes.length;
      await page.keyboard.press(`${MOD}+s`);
      const ask = page.getByRole("alertdialog", { name: `Save the open files before the flow moves to ${MOVED}?` });
      await expect(ask).toBeVisible();
      await expect(ask.getByRole("button", { name: "Cancel" })).toBeVisible();
      await expect(ask.getByRole("button", { name: "Discard file changes" })).toBeVisible();
      await shot(page, "editor-namespace-change-dialog");
      expect(writes.length, "nothing is written while the question is open").toBe(n);
      await ask.getByRole("button", { name: "Save files first" }).click();
      await expect(ask).toBeHidden({ timeout: 15_000 });
      await expect.poll(() => writes.slice(n).map((w) => [w.kind, w.namespace, w.path])).toEqual([["file", TEAM, C], ["flow", "", null]]);
      expect(await (await page.request.get(`${nsApi(TEAM)}/files?path=${encodeURIComponent(C)}`)).text()).toContain("# saved before the move");
      expect((await page.request.get(`${nsApi(MOVED)}/files/stats?path=${encodeURIComponent(C)}`)).status(), "nothing written into the new namespace").toBe(404);
      await expect(page.getByText(MOVED, { exact: true }).first()).toBeVisible();
      // The file tabs of the old namespace closed with the move.
      await expect(fileEditor(page, "c.py")).toHaveCount(0);
    });

    // ── 8. Dependencies: the subflow edge between company.other and company.team ─────────
    await test.step("dependencies: the subflow edge", async () => {
      const deps = await (await page.request.get(`${nsApi(TEAM)}/dependencies`)).json();
      expect(deps.edges).toContainEqual({ source: caller.id, target: child.id, kind: "subflow" });
      await page.goto(`${base}/namespaces/${TEAM}/dependencies`);
      const canvasEl = page.locator('.react-flow[aria-label="Dependencies of the namespace"]');
      await expect(canvasEl).toBeVisible({ timeout: 30_000 });
      await expect(canvasEl.locator(`[data-edge="${caller.id} -> ${child.id}"]`)).toBeAttached();
      await expect(canvasEl.locator(`[data-flow="${caller.id}"]`)).toBeVisible();
      await expect(canvasEl.locator(`[data-flow="${child.id}"]`)).toBeVisible();
      await shot(page, "dependencies-light");
      await page.getByRole("button", { name: "List view" }).click();
      const list = page.getByRole("table", { name: "Flows of the namespace and their dependencies" });
      await expect(list.locator(`tr[data-flow="${caller.id}"]`)).toContainText(`${TEAM}.${CHILD_ID} (Subflow)`);
      await expect(list.locator(`tr[data-flow="${child.id}"]`)).toContainText(`${OTHER}.${CALLER_ID} (Subflow)`);
    });
  });
});
