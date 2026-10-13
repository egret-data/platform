import { test, expect, type Locator, type Page } from "@playwright/test";
import { csrf, firstWorkspace, loginLocalAdmin, postJson } from "./helpers";

/**
 * W7c (workflow UI, topology) end to end: the editor's Topology tab follows
 * the YAML after its debounce and keeps the last graph under a banner while
 * the YAML is broken; selecting a node moves the Code cursor; the execution's
 * Topology tab shows states changing while the flow runs and the final ones
 * after; a `Loop` body node lists its three iterations, one failed; a
 * `Subflow` expands in place; and the canvas is measured in light and dark
 * mode and in a panel 600 px wide (spec `docs/en/specs/2026-10-13-workflow-w7c-topology-design.md`
 * §8, FE-TEST-2).
 *
 * Opt-in (`EGRET_E2E_WORKFLOWS=1`). Like `workflows-executions.spec.ts` it
 * needs a backend with the workflow worker on, signs in once as the
 * break-glass local admin, and uses only core tasks: default settings allow no
 * container image.
 *
 * HOW TO RUN IT, on a stack of its own and never the shared dev stack (no
 * Postgres :5432, no shared bucket, no kind cluster). From the egret repo root:
 *
 * ```bash
 * # 1. A database of its own on the branch's Postgres (:5444), a private RustFS with one bucket.
 * #    The branch's Postgres, once, if it does not exist yet (any Postgres you own will do):
 * docker run -d --name egret-w7c-postgres -p 5444:5432 -e POSTGRES_USER=egret -e POSTGRES_PASSWORD=egret postgres:16
 * docker exec egret-w7c-postgres psql -U egret -d postgres -c "CREATE DATABASE egret_w7c_live"
 * docker run -d --name w7c-rustfs -p 9467:9000 \
 *   -e RUSTFS_ACCESS_KEY=egret -e RUSTFS_SECRET_KEY=egretsecret rustfs/rustfs:1.0.1
 * curl --aws-sigv4 "aws:amz:us-east-1:s3" --user egret:egretsecret -X PUT http://localhost:9467/warehouse
 *
 * # 2. The backend on :8207, pointed only at those two.
 * export DATABASE_URL=postgres://egret:egret@localhost:5444/egret_w7c_live
 * export EGRET_DATABASE__URL=$DATABASE_URL EGRET_SERVER__PORT=8207 EGRET_SERVER__HOST=127.0.0.1
 * export EGRET_OIDC__ENABLED=false EGRET_WORKFLOW__WORKER_ENABLED=true
 * export EGRET_STORAGE__ENDPOINT=http://localhost:9467 EGRET_STORAGE__BUCKET=warehouse
 * export EGRET_STORAGE__ACCESS_KEY=egret EGRET_STORAGE__SECRET_KEY=egretsecret
 * # `init` generates a random password unless one is given; the spec signs in with this one.
 * export EGRET_ADMIN_PASSWORD='Admin12345!'
 * # Only while iterating, and only on this private backend: sign-in is limited to 5 per 5 minutes.
 * # export EGRET_RATE_LIMIT__LOGIN_MAX_ATTEMPTS=200
 * cargo build -p egret-bin
 * ./target/debug/egret migrate && ./target/debug/egret init   # admin@egret.local
 * ./target/debug/egret serve
 *
 * # 3. The frontend on :5217, proxying /api to :8207 (the committed config next to this spec).
 * cd frontend && EGRET_E2E_VITE_PORT=5217 EGRET_E2E_API=http://localhost:8207 \
 *   npx vite --config tests/e2e/vite.e2e.config.mts
 *
 * # 4. The spec.
 * cd frontend && EGRET_E2E_WORKFLOWS=1 EGRET_E2E_BASE_URL=http://localhost:5217 \
 *   npx playwright test workflows-topology
 *
 * #    Vite reloads every open page when it optimises a dependency on first use; the spec's
 * #    `beforeAll` warms those modules up on a throwaway page first.
 *
 * # 5. Afterwards: stop both servers, `docker rm -f -v w7c-rustfs`, and drop the database by its name:
 * docker exec egret-w7c-postgres psql -U egret -d postgres -c "DROP DATABASE egret_w7c_live"
 * ```
 */
const ENABLED = process.env.EGRET_E2E_WORKFLOWS === "1";

/** W7b's polling (`executions/polling.ts`): a live read every 2 s, at most 30 s apart while reads fail. */
const POLL_MS = 2_000;
const POLL_MAX_MS = 30_000;

const RUN = Date.now().toString(36);
const NAMESPACE = "e2e.w7c";
const CHILD_ID = `w7c-child-${RUN}`;
const FLOW_ID = `w7c-e2e-${RUN}`;

const CHILD = `id: ${CHILD_ID}
namespace: ${NAMESPACE}
tasks:
  - id: child_log
    type: io.kestra.plugin.core.log.Log
    message: from the child
`;

/**
 * Every construct of §8's end to end, and the flow the Kestra comparison
 * (§9) ran unchanged but for its ids: a disabled trigger; a `Sequential`
 * holding a `Sleep` (so a state is seen RUNNING) and an `If`; a `Parallel`;
 * a `Subflow` to {@link CHILD}; a `Loop` over three values whose middle one
 * fails (`concurrencyLimit: 0` starts all three: with 1 the first failure
 * stops the rest); and the flow's `errors:` branch, which the failure runs.
 */
const SOURCE = `id: ${FLOW_ID}
namespace: ${NAMESPACE}
triggers:
  - id: nightly
    type: io.kestra.plugin.core.trigger.Schedule
    cron: "0 3 * * *"
    disabled: true
tasks:
  - id: prepare
    type: io.kestra.plugin.core.flow.Sequential
    tasks:
      - id: hello
        type: io.kestra.plugin.core.log.Log
        message: hello
      - id: nap
        type: io.kestra.plugin.core.flow.Sleep
        duration: PT8S
      - id: check
        type: io.kestra.plugin.core.flow.If
        condition: "{{ 1 == 1 }}"
        then:
          - id: when_true
            type: io.kestra.plugin.core.log.Log
            message: then branch
        else:
          - id: when_false
            type: io.kestra.plugin.core.log.Log
            message: else branch
  - id: fan
    type: io.kestra.plugin.core.flow.Parallel
    tasks:
      - id: left
        type: io.kestra.plugin.core.log.Log
        message: left
      - id: right
        type: io.kestra.plugin.core.log.Log
        message: right
  - id: call
    type: io.kestra.plugin.core.flow.Subflow
    namespace: ${NAMESPACE}
    flowId: ${CHILD_ID}
    wait: true
    transmitFailed: true
  - id: each
    type: io.kestra.plugin.core.flow.Loop
    values: ["a", "b", "c"]
    concurrencyLimit: 0
    tasks:
      - id: item
        type: io.kestra.plugin.core.execution.Fail
        condition: "{{ item.value == 'b' }}"
        errorMessage: b fails on purpose
errors:
  - id: alert
    type: io.kestra.plugin.core.log.Log
    message: the flow failed
`;

/** The 1-based line of `- id: {id}` in {@link SOURCE}: where the Code cursor lands for that task. */
const lineOf = (id: string): number => SOURCE.split("\n").findIndex((l) => l.trim() === `- id: ${id}`) + 1;

/** The flow editor's Monaco (its `ariaLabel` is "Flow source"). */
const codeEditor = (page: Page): Locator => page.locator(".monaco-editor").filter({ has: page.getByRole("textbox", { name: "Flow source" }) });

/** Playwright's Chromium on macOS, like on Linux, edits with Control. */
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

/** Replace the whole text of the flow editor; returns when the paste was dispatched. */
async function replaceSource(page: Page, text: string): Promise<number> {
  await codeEditor(page).locator(".view-lines").click();
  await page.keyboard.press(`${MOD}+A`);
  await paste(page, text);
  return Date.now();
}

/** The line Monaco shows as the cursor's. */
const activeLine = (page: Page): Locator => codeEditor(page).locator(".line-numbers.active-line-number");

/** The canvas (xyflow's root carries the view's label). */
const canvas = (page: Page): Locator => page.locator('.react-flow[aria-label="Topology of the flow"]');
/** A node's button, by uid. */
const nodeButton = (page: Page, uid: string): Locator => canvas(page).locator(`button[data-uid="${uid}"]`);
/** A node's card (the button, its facts, its state chip). */
const nodeCard = (page: Page, uid: string): Locator => canvas(page).locator("[data-node-card]", { has: page.locator(`button[data-uid="${uid}"]`) });
const cardState = (page: Page, uid: string, state: string): Locator => nodeCard(page, uid).locator(`[data-state="${state}"]`);

const headerState = (page: Page, state: string): Locator =>
  page.locator("header", { has: page.getByTestId("execution-summary") }).locator(`[data-state="${state}"]`).first();

interface CanvasMeasure {
  canvas: { width: number; height: number };
  /** The fitted zoom (xyflow's viewport scale): how small the cards are drawn. */
  zoom: number;
  cards: number;
  /** Cards, cluster boxes and markers whose box leaves the canvas. */
  outside: string[];
  /** Labels (task ids, cluster headers) whose text is cut. */
  clipped: string[];
  ring: {
    uid: string | null;
    focusVisible: boolean;
    shadow: string;
    /** The box-shadow layer with a spread: Tailwind's `ring-2`. */
    ringColour: string | null;
    cardBackground: string;
    /** WCAG contrast of the ring against its card (non-text UI: at least 3). */
    contrast: number | null;
    clippedBy: string[];
  };
}

/**
 * Fit, then measure in one frame: every card, cluster box and marker inside
 * the canvas; no task id or cluster header cut; and a node focused by the
 * keyboard (Home on the first card) drawing a visible ring, at 3:1 or more
 * against its card, that no ancestor up to the canvas clips.
 */
async function measureCanvas(page: Page): Promise<CanvasMeasure> {
  await page.getByRole("button", { name: "Fit view" }).click();
  // xyflow's fit animates nothing by default, but the viewport is applied a frame later.
  await page.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))));
  const first = canvas(page).locator("button[data-uid]").first();
  await first.focus();
  await page.keyboard.press("End");
  await page.keyboard.press("Home");
  return canvas(page).evaluate((root) => {
    const box = root.getBoundingClientRect();
    const outside: string[] = [];
    for (const el of root.querySelectorAll<HTMLElement>("[data-node-card], [data-cluster], [data-marker]")) {
      const r = el.getBoundingClientRect();
      if (r.left < box.left - 0.5 || r.top < box.top - 0.5 || r.right > box.right + 0.5 || r.bottom > box.bottom + 0.5) {
        outside.push(el.dataset.cluster ?? el.dataset.marker ?? el.querySelector<HTMLElement>("[data-uid]")?.dataset.uid ?? "?");
      }
    }
    const labels = root.querySelectorAll<HTMLElement>("[data-node-card] .font-mono, [data-cluster] .truncate");
    const clipped = [...labels].filter((e) => e.scrollWidth > e.clientWidth + 0.5).map((e) => e.textContent ?? "");
    const active = document.activeElement as HTMLElement;
    const shadow = getComputedStyle(active).boxShadow;
    const clippedBy: string[] = [];
    for (let el = active.parentElement; el && el !== root; el = el.parentElement) {
      const cs = getComputedStyle(el);
      if (cs.overflow !== "visible" && cs.overflow !== "") clippedBy.push(`${el.className} (${cs.overflow})`);
    }
    const card = active.closest<HTMLElement>("[data-node-card]");
    const inner = card?.querySelector<HTMLElement>(".bg-card") ?? card;
    const cardBackground = inner ? getComputedStyle(inner).backgroundColor : "";
    // The layer with a non-zero spread (`0px 0px 0px 2px`); the others are Tailwind's transparent placeholders.
    const ringColour = shadow.split(/,(?![^(]*\))/).map((l) => l.trim()).find((l) => /\b0px 0px 0px [1-9]/.test(l))?.replace(/\s+(-?\d+(\.\d+)?px\s*){4}$/, "") ?? null;
    // Any CSS colour to sRGB through a canvas, then WCAG relative luminance.
    const ctx2d = document.createElement("canvas").getContext("2d")!;
    const luminance = (colour: string): number => {
      ctx2d.clearRect(0, 0, 1, 1);
      ctx2d.fillStyle = colour;
      ctx2d.fillRect(0, 0, 1, 1);
      const [r, g, b] = ctx2d.getImageData(0, 0, 1, 1).data;
      const lin = (c: number) => ((c /= 255) <= 0.03928 ? c / 12.92 : ((c + 0.055) / 1.055) ** 2.4);
      return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
    };
    let contrast: number | null = null;
    if (ringColour && cardBackground) {
      const [a, b] = [luminance(ringColour), luminance(cardBackground)].sort((x, y) => y - x);
      contrast = Math.round(((a + 0.05) / (b + 0.05)) * 100) / 100;
    }
    const transform = getComputedStyle(root.querySelector(".react-flow__viewport")!).transform;
    const zoom = transform === "none" ? 1 : new DOMMatrix(transform).a;
    return {
      canvas: { width: Math.round(box.width), height: Math.round(box.height) },
      zoom: Math.round(zoom * 1000) / 1000,
      cards: root.querySelectorAll("[data-node-card]").length,
      outside,
      clipped,
      ring: {
        uid: active.dataset.uid ?? null,
        focusVisible: active.matches(":focus-visible"),
        shadow,
        ringColour,
        cardBackground,
        contrast,
        clippedBy,
      },
    };
  });
}

/** `READABLE_ZOOM` in `TopologyView.tsx`: the first view's least zoom. */
const READABLE_ZOOM = 0.8;

interface FirstView {
  zoom: number;
  /** The flow's start marker is inside the canvas. */
  startInView: boolean;
  /** The first task's id, as drawn: its font size times the zoom, and its box's height on screen. */
  firstTaskFontPx: number;
  firstTaskTextHeight: number;
  firstTaskInView: boolean;
}

/** The view as the canvas first shows it (no Fit pressed): its zoom, and how the flow's start and first task are drawn. */
async function measureFirstView(page: Page, firstTask: string): Promise<FirstView> {
  // The first view is placed once the canvas has its size; wait until a view is applied.
  await expect.poll(() => canvas(page).evaluate((root) => getComputedStyle(root.querySelector(".react-flow__viewport")!).transform !== "none")).toBe(true);
  await page.evaluate(() => new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r))));
  return canvas(page).evaluate((root, uid) => {
    const box = root.getBoundingClientRect();
    const inside = (r: DOMRect) => r.left >= box.left - 0.5 && r.top >= box.top - 0.5 && r.right <= box.right + 0.5 && r.bottom <= box.bottom + 0.5;
    const transform = getComputedStyle(root.querySelector(".react-flow__viewport")!).transform;
    const zoom = new DOMMatrix(transform).a;
    const start = root.querySelector<HTMLElement>('[data-marker="start:flow"]')!.getBoundingClientRect();
    const label = root.querySelector<HTMLElement>(`button[data-uid="${uid}"] .font-mono`)!;
    const labelBox = label.getBoundingClientRect();
    return {
      zoom: Math.round(zoom * 1000) / 1000,
      startInView: inside(start),
      firstTaskFontPx: Math.round(Number.parseFloat(getComputedStyle(label).fontSize) * zoom * 100) / 100,
      firstTaskTextHeight: Math.round(labelBox.height * 100) / 100,
      firstTaskInView: inside(labelBox),
    };
  }, firstTask);
}

function expectReadableFirstView(v: FirstView, what: string): void {
  console.log(`first view [${what}]: ${JSON.stringify(v)}`);
  expect(v.zoom, `${what}: first zoom`).toBeGreaterThanOrEqual(READABLE_ZOOM - 0.001);
  expect(v.startInView, `${what}: the flow's start is in view`).toBe(true);
  expect(v.firstTaskFontPx, `${what}: first task's text size`).toBeGreaterThanOrEqual(10);
  expect(v.firstTaskTextHeight, `${what}: first task's text box`).toBeGreaterThanOrEqual(10);
}

/** The viewport's transform, to show a poll leaves the user's view alone. */
const viewportTransform = (page: Page): Promise<string> =>
  canvas(page).evaluate((root) => getComputedStyle(root.querySelector(".react-flow__viewport")!).transform);

function expectCanvasFits(m: CanvasMeasure, what: string): void {
  expect(m.cards, `${what}: cards drawn`).toBeGreaterThan(5);
  expect(m.outside, `${what}: boxes outside the canvas`).toEqual([]);
  expect(m.clipped, `${what}: labels cut`).toEqual([]);
  expect(m.ring.focusVisible, `${what}: the focused node is :focus-visible`).toBe(true);
  expect(m.ring.shadow, `${what}: focus ring`).not.toBe("none");
  expect(m.ring.ringColour, `${what}: ring colour`).not.toBeNull();
  expect(m.ring.contrast, `${what}: ring against its card`).toBeGreaterThanOrEqual(3);
  expect(m.ring.clippedBy, `${what}: an ancestor clips the ring`).toEqual([]);
}

/** Light or dark, through the app's own preference (`ThemeProvider`, `egret-theme`), then a reload. */
async function setTheme(page: Page, theme: "light" | "dark"): Promise<void> {
  await page.evaluate((t) => localStorage.setItem("egret-theme", t), theme);
  await page.reload();
  await expect(page.locator("html")).toHaveClass(theme === "dark" ? /\bdark\b/ : /^(?!.*\bdark\b)/);
}

/** `EGRET_E2E_SHOTS=<dir>`: also save a screenshot of each measured state there (for a report). */
const SHOTS = process.env.EGRET_E2E_SHOTS;

async function record(page: Page, key: string, m: CanvasMeasure & { panelWidth?: number }): Promise<void> {
  measured[key] = m;
  if (SHOTS) await page.screenshot({ path: `${SHOTS}/${key.replace(/\W+/g, "-")}.png` });
  expectCanvasFits(m, key);
}

/** Every measurement taken, printed as it is taken (for the report), even when a later step fails. */
const measured = new Proxy({} as Record<string, CanvasMeasure & { panelWidth?: number }>, {
  set(target, key: string, value: CanvasMeasure & { panelWidth?: number }) {
    target[key] = value;
    console.log(`canvas measurement [${key}]: ${JSON.stringify(value)}`);
    return true;
  },
});

test.describe("W7c topology: editor, execution, Loop, Subflow, measured", () => {
  test.skip(!ENABLED, "opt-in: set EGRET_E2E_WORKFLOWS=1 with a private local stack up (see the comment above)");
  test.setTimeout(300_000);

  // Vite optimises a dependency the first time a module needs it (Monaco's worker, xyflow) and then
  // reloads every open page, which would land mid-test. Load those modules once on a throwaway
  // page first, until a load passes without a reload.
  test.beforeAll(async ({ browser }, testInfo) => {
    test.setTimeout(120_000);
    const page = await browser.newPage({ baseURL: testInfo.project.use.baseURL });
    try {
      for (let i = 0; i < 4; i++) {
        await page.goto("/login");
        await page.evaluate(() => Object.assign(window, { egretWarm: true }));
        await page
          .evaluate((paths) => Promise.all(paths.map((p) => import(/* @vite-ignore */ p))), [
            "/src/lib/monaco.ts",
            "/src/components/workflows/editor/registerEditorTabs.tsx",
            "/src/components/workflows/topology/TopologyTab.tsx",
            "/src/components/workflows/executions/topology/ExecutionTopologyTab.tsx",
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

  test("editor graph follows the YAML → stale banner → cursor → live states → Loop iterations → Subflow → measured", async ({ page, context }) => {
    // Every unsaved-graph request, to show the debounce.
    const graphPosts: number[] = [];
    const executionGraphReads: number[] = [];
    page.on("request", (r) => {
      if (r.method() === "POST" && /\/workflows\/flows\/graph(\?|$)/.test(r.url())) graphPosts.push(Date.now());
      if (/\/workflows\/executions\/[^/]+\/graph(\?|$)/.test(r.url())) executionGraphReads.push(Date.now());
    });

    page.on("console", (m) => { if (m.type() === "error" || m.type() === "warning") console.log(`[console ${m.type()}] ${m.text().slice(0, 400)}`); });
    page.on("pageerror", (e) => console.log(`[pageerror] ${e.message}`));
    await loginLocalAdmin(page);
    await setTheme(page, "light");
    const ws = (await firstWorkspace(page.request)).slug;
    const api = `/api/ws/${ws}/workflows`;
    const h = await csrf(context);
    const base = `/explore/${ws}/workflows`;

    // ── 1. The two flows, created through the API (W7a drives the editor) ──
    const child = await postJson(page.request, `${api}/flows`, h, { source: CHILD });
    expect(child.validation.errors, JSON.stringify(child.validation)).toEqual([]);
    const flow = await postJson(page.request, `${api}/flows`, h, { source: SOURCE });
    expect(flow.validation.errors, JSON.stringify(flow.validation)).toEqual([]);

    // ── 2. The editor's Topology tab, opened beside the code ────────────
    await page.goto(`${base}/flows/${flow.id}/edit`);
    await expect(codeEditor(page)).toBeVisible({ timeout: 30_000 });
    await page.getByRole("button", { name: "Reopen a tab in panel 2" }).click();
    await page.getByRole("menuitem", { name: "Topology" }).click();
    await expect(nodeButton(page, "tasks.prepare.check.when_true")).toBeVisible({ timeout: 30_000 });
    // The first view is readable (a fit of this flow is about 0.15 here), horizontally and vertically.
    expectReadableFirstView(await measureFirstView(page, "tasks.prepare"), "editor first view LR");
    await page.getByRole("button", { name: "Vertical" }).click();
    expectReadableFirstView(await measureFirstView(page, "tasks.prepare"), "editor first view TB");
    await page.getByRole("button", { name: "Horizontal" }).click();
    // The steps below click nodes all over the graph: Fit puts every one of them on the canvas.
    await page.getByRole("button", { name: "Fit view" }).click();
    // Every construct is there: a trigger, the clusters, the errors branch, the Loop body.
    for (const uid of ["triggers.nightly", "tasks.prepare.nap", "tasks.fan.left", "tasks.fan.right", "tasks.call", "tasks.each.item", "errors.alert"]) {
      await expect(nodeButton(page, uid), uid).toBeAttached();
    }
    for (const c of ["cluster:tasks.prepare", "cluster:tasks.prepare.check", "cluster:tasks.fan", "cluster:tasks.each", "cluster:triggers"]) {
      await expect(canvas(page).locator(`[data-cluster="${c}"]`), c).toBeAttached();
    }

    // Edit: rename `right`; the graph follows, and only after the 1 s debounce.
    const renamed = SOURCE.replace("- id: right\n", "- id: right_side\n");
    const before = graphPosts.length;
    const editedAt = await replaceSource(page, renamed);
    await expect(nodeButton(page, "tasks.fan.right_side")).toBeAttached({ timeout: 10_000 });
    await expect(nodeButton(page, "tasks.fan.right")).toHaveCount(0);
    const firstAfterEdit = graphPosts.slice(before)[0];
    expect(firstAfterEdit - editedAt, "the graph is asked for 1 s after the edit, not at once").toBeGreaterThanOrEqual(950);

    // Break the YAML: the banner shows, and the last graph stays.
    await replaceSource(page, `${renamed}  - id: [broken\n`);
    await expect(page.getByRole("status").filter({ hasText: "The graph is out of date: the YAML has an error" })).toBeVisible({ timeout: 10_000 });
    await expect(nodeButton(page, "tasks.fan.right_side")).toBeAttached();
    await expect(nodeButton(page, "tasks.prepare.check.when_true")).toBeAttached();

    // Back to the saved text: the banner goes, nothing is unsaved.
    await replaceSource(page, SOURCE);
    await expect(nodeButton(page, "tasks.fan.right")).toBeAttached({ timeout: 10_000 });
    await expect(page.getByText("The graph is out of date: the YAML has an error")).toBeHidden();
    await expect(page.getByText("Unsaved changes", { exact: true })).toBeHidden();

    // ── 3. Select a node: the Code cursor moves to its task ─────────────
    await codeEditor(page).locator(".view-lines").click();
    await page.keyboard.press(`${MOD}+Home`);
    await expect(activeLine(page)).toHaveText("1");
    await nodeButton(page, "tasks.prepare.check.when_true").click();
    await expect(activeLine(page)).toHaveText(String(lineOf("when_true")));
    await nodeButton(page, "errors.alert").click();
    await expect(activeLine(page)).toHaveText(String(lineOf("alert")));

    // The editor expands a Subflow from the child's draft.
    await page.getByRole("button", { name: "Expand subflow call" }).click();
    await expect(nodeButton(page, "tasks.call/tasks.child_log")).toBeAttached({ timeout: 10_000 });
    await expect(canvas(page).locator('[data-cluster="cluster:tasks.call"]')).toBeAttached();
    // An expansion keeps the view (the same flow); Fit brings the child's cluster onto the canvas.
    await page.getByRole("button", { name: "Fit view" }).click();
    // Selecting a task of the child flow says where it lives instead of moving the cursor.
    await nodeButton(page, "tasks.call/tasks.child_log").click();
    await expect(page.getByRole("status").filter({ hasText: "child_log is a task of the Subflow call's flow" })).toBeVisible();

    // ── 6a. The editor's panel, measured at 600 px, light then dark ─────
    const panel = page.getByRole("tabpanel", { name: "Topology", exact: true });
    let viewport = 1280;
    for (let i = 0; i < 3; i++) {
      const w = (await panel.boundingBox())!.width;
      if (Math.abs(w - 600) <= 4) break;
      // Panel 2 holds 40 % of the row.
      viewport = Math.round(viewport + (600 - w) / 0.4);
      await page.setViewportSize({ width: viewport, height: 820 });
    }
    const panelWidth = Math.round((await panel.boundingBox())!.width);
    expect(Math.abs(panelWidth - 600), `the Topology panel is ${panelWidth} px`).toBeLessThanOrEqual(4);
    await record(page, "editor 600px light", { ...(await measureCanvas(page)), panelWidth });
    // The vertical layout fits the narrow panel as well.
    await page.getByRole("button", { name: "Vertical" }).click();
    await record(page, "editor 600px light vertical", { ...(await measureCanvas(page)), panelWidth });
    await page.getByRole("button", { name: "Horizontal" }).click();

    await setTheme(page, "dark");
    await expect(codeEditor(page)).toBeVisible({ timeout: 30_000 });
    await expect(nodeButton(page, "tasks.prepare.check.when_true")).toBeAttached({ timeout: 30_000 });
    await record(page, "editor 600px dark", { ...(await measureCanvas(page)), panelWidth: Math.round((await panel.boundingBox())!.width) });
    await setTheme(page, "light");
    await page.setViewportSize({ width: 1280, height: 820 });

    // ── 4. Run: the execution's Topology shows states changing while it runs ──
    const started = await postJson(page.request, `${api}/flows/${flow.id}/executions`, h, { inputs: {} });
    const execId: string = started.id;
    await page.goto(`${base}/executions/${execId}/topology`);
    await expect(nodeButton(page, "tasks.prepare.nap")).toBeVisible({ timeout: 30_000 });
    expectReadableFirstView(await measureFirstView(page, "tasks.prepare"), "execution first view LR");
    const firstTransform = await viewportTransform(page);
    // The 8 s Sleep is seen running, then done, without a reload.
    await expect(cardState(page, "tasks.prepare.nap", "RUNNING")).toBeVisible({ timeout: 20_000 });
    const readsWhileRunning = executionGraphReads.length;
    await expect(cardState(page, "tasks.prepare.nap", "SUCCESS")).toBeVisible({ timeout: 30_000 });
    expect(executionGraphReads.length, "the graph was polled while the execution ran").toBeGreaterThan(readsWhileRunning);
    // The polls did not move the view.
    expect(await viewportTransform(page), "the view after the polls").toBe(firstTransform);

    // The final states, once the execution is over.
    await expect(headerState(page, "FAILED")).toBeVisible({ timeout: 60_000 });
    await expect(cardState(page, "tasks.each", "FAILED")).toBeVisible({ timeout: 15_000 });
    await expect(cardState(page, "tasks.each.item", "FAILED")).toBeVisible();
    await expect(nodeCard(page, "tasks.each.item")).toContainText("2 of 3 iterations done, 1 failed");
    await expect(cardState(page, "tasks.prepare.check.when_true", "SUCCESS")).toBeVisible();
    await expect(cardState(page, "tasks.fan.left", "SUCCESS")).toBeVisible();
    await expect(cardState(page, "tasks.call", "SUCCESS")).toBeVisible();
    await expect(cardState(page, "errors.alert", "SUCCESS")).toBeVisible();
    // The branch that did not run is dimmed, with no state.
    await expect(nodeCard(page, "tasks.prepare.check.when_false")).toHaveAttribute("data-dimmed", "true");
    await expect(nodeCard(page, "tasks.prepare.check.when_false").locator("[data-state]")).toHaveCount(0);
    // Terminal: the polling stops. The tab reads once more when the execution ends; then, over a
    // window longer than W7b's backoff ceiling (`POLL_MAX_MS`, 30 s; a live poll is every 2 s),
    // there is no read at all.
    await page.waitForTimeout(POLL_MS + 1_000);
    const readsAtEnd = executionGraphReads.length;
    await page.waitForTimeout(POLL_MAX_MS + POLL_MS);
    expect(executionGraphReads.length - readsAtEnd, "graph reads after the end").toBe(0);

    // The vertical first view of the execution, then back, and Fit so every node can be clicked.
    await page.getByRole("button", { name: "Vertical" }).click();
    expectReadableFirstView(await measureFirstView(page, "tasks.prepare"), "execution first view TB");
    await page.getByRole("button", { name: "Horizontal" }).click();
    await page.getByRole("button", { name: "Fit view" }).click();

    // ── 5. The Loop body node: three iterations, one failed ─────────────
    await nodeButton(page, "tasks.each.item").click();
    await expect(page).toHaveURL(/[?&]node=tasks\.each\.item/);
    const iterations = page.getByTestId("loop-iterations");
    await expect(iterations).toBeVisible();
    await expect(iterations).toContainText("2 of 3 iterations done, 1 failed");
    const rows = iterations.locator("tbody tr");
    await expect(rows).toHaveCount(3);
    await expect(rows.locator('[data-state="FAILED"]')).toHaveCount(1);
    await expect(rows.locator('[data-state="SUCCESS"]')).toHaveCount(2);
    await expect(rows.nth(1).locator('[data-state="FAILED"]')).toBeVisible();
    // Each row links to its iteration's child execution, on its Topology tab at the same task.
    await expect(rows.nth(1).getByRole("link")).toHaveAttribute("href", /\/workflows\/executions\/[0-9a-f-]{36}\/topology\?node=tasks\.item$/);
    await iterations.getByRole("button", { name: "Close the iterations" }).click();
    await expect(iterations).toBeHidden();

    // ── 5b. Expand the Subflow: the child's tasks, with their states ────
    await page.getByRole("button", { name: "Expand subflow call" }).click();
    await expect(page).toHaveURL(/[?&]expand=tasks\.call/);
    await expect(cardState(page, "tasks.call/tasks.child_log", "SUCCESS")).toBeVisible({ timeout: 15_000 });
    await page.getByRole("button", { name: "Fit view" }).click();
    await nodeButton(page, "tasks.call/tasks.child_log").click();
    await expect(page.getByRole("link", { name: "Open the child execution" })).toHaveAttribute("href", /\/workflows\/executions\/[0-9a-f-]{36}\/topology\?node=tasks\.child_log$/);

    // ── 6b. The execution's canvas, measured, light then dark ───────────
    await record(page, "execution light", await measureCanvas(page));
    await setTheme(page, "dark");
    await expect(cardState(page, "tasks.call/tasks.child_log", "SUCCESS")).toBeVisible({ timeout: 30_000 });
    await record(page, "execution dark", await measureCanvas(page));
    // The list view, the canvas's accessible equivalent, carries the same states.
    await page.getByRole("button", { name: "List view" }).click();
    const grid = page.getByRole("treegrid");
    await expect(grid).toBeVisible();
    await expect(grid.getByRole("row").filter({ has: page.getByText("item", { exact: true }) }).locator('[data-state="FAILED"]')).toBeVisible();
    await setTheme(page, "light");
  });
});
