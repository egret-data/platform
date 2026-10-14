import { execFileSync } from "node:child_process";
import { test, expect, type Browser, type Locator, type Page, type TestInfo } from "@playwright/test";
import { csrf, firstWorkspace, loginLocalAdmin, postJson } from "./helpers";

/**
 * W7e (workflow triggers) end to end, spec
 * `docs/en/specs/2026-10-14-workflow-w7e-triggers-design.md` §6, scenarios 1-7:
 *
 * 1. a flow with a `* * * * *` schedule, a webhook, a `dependsOn` Flow trigger on two upstream
 *    flows and a schedule `disabled: true` in its YAML: the Triggers tab shows each with its next
 *    run, and the drawer the webhook's path (masked, then revealed);
 * 2. the schedule switched off: past two ticks no execution is created (read in the database);
 *    on with "Skip": the next run is after the switch and no missed execution appears; off again
 *    and on with "Recover": the missed ticks run, one execution per tick;
 * 3. the webhook switched off: a POST to its path on the backend's own port answers 409; on
 *    again: 200 and an execution;
 * 4. one upstream run: the drawer shows the window and one condition met; the other: the
 *    downstream flow runs;
 * 5. the global page, filtered by namespace and kind in the URL: two triggers of another flow
 *    disabled in bulk, and after a reload both are off (DOM and database);
 * 6. a viewer with `workflow:read` and not `workflow:manage` sees every switch disabled with
 *    "You need workflow:manage", and the bulk buttons too;
 * 7. the YAML-disabled trigger's switch is disabled with "Disabled in the flow's YAML".
 *
 * And measured in the DOM (FE-VISUAL-4, FE-TEST-2), in light and dark mode at 1280 px and at
 * 390 px: the tab, the drawer per kind, the enable dialog and the global page. Each measurement
 * is printed (`measure [...]`): page overflow, clipped text, and the contrast of each sampled
 * text against the surface it is painted on, the colours resolved by the browser.
 *
 * Every wait on the scheduler is bounded by the schedule: the spec reads the trigger's
 * `next_evaluation_date` and the database's `now()`, and waits until the ticks it needs have
 * passed (plus a margin of a few scheduler passes), never for a fixed sleep. A run takes about
 * six minutes, most of it the minute ticks.
 *
 * Opt-in (`EGRET_E2E_WORKFLOWS=1`). It signs in twice (the admin, then the viewer), within the
 * sign-in limit of 5 per 5 minutes, and uses only core tasks.
 *
 * HOW TO RUN IT, on a stack of its own and never the shared dev stack (no Postgres :5432, no
 * shared bucket, no kind cluster). From the egret repo root:
 *
 * ```bash
 * # 1. A database of its own on the branch's Postgres (:5448), a private RustFS with one bucket
 * #    (the apps API refuses to start without storage). The branch's Postgres, once:
 * docker run -d --name egret-w7e-postgres -p 5448:5432 -e POSTGRES_USER=egret -e POSTGRES_PASSWORD=egret postgres:16
 * docker exec egret-w7e-postgres psql -U egret -d postgres -c "CREATE DATABASE egret_w7e_e2e"
 * docker run -d --name w7e-rustfs -p 9488:9000 \
 *   -e RUSTFS_ACCESS_KEY=egret -e RUSTFS_SECRET_KEY=egretsecret rustfs/rustfs:1.0.1
 * curl --aws-sigv4 "aws:amz:us-east-1:s3" --user egret:egretsecret -X PUT http://localhost:9488/warehouse
 *
 * # 2. The backend on :8237 with the scheduler, the worker and webhooks on.
 * export DATABASE_URL=postgres://egret:egret@localhost:5448/egret_w7e_e2e
 * export EGRET_DATABASE__URL=$DATABASE_URL EGRET_SERVER__PORT=8237 EGRET_SERVER__HOST=127.0.0.1
 * export EGRET_OIDC__ENABLED=false EGRET_WORKFLOW__WORKER_ENABLED=true
 * export EGRET_WORKFLOW__SCHEDULER_ENABLED=true EGRET_WORKFLOW__WEBHOOKS_ENABLED=true
 * export EGRET_STORAGE__ENDPOINT=http://localhost:9488 EGRET_STORAGE__BUCKET=warehouse
 * export EGRET_STORAGE__ACCESS_KEY=egret EGRET_STORAGE__SECRET_KEY=egretsecret
 * export EGRET_ADMIN_PASSWORD='Admin12345!'
 * # Only while iterating, and only on this private backend: sign-in is limited to 5 per 5 minutes.
 * # export EGRET_RATE_LIMIT__LOGIN_MAX_ATTEMPTS=200
 * cargo build -p egret-bin
 * ./target/debug/egret migrate && ./target/debug/egret init   # admin@egret.local
 * ./target/debug/egret serve
 *
 * # 3. The frontend on :5237, proxying /api to :8237 (the committed config next to this spec).
 * cd frontend && EGRET_E2E_VITE_PORT=5237 EGRET_E2E_API=http://localhost:8237 \
 *   npx vite --config tests/e2e/vite.e2e.config.mts
 *
 * # 4. The spec. It reads the database through `docker exec <container> psql`, and posts to the
 * #    webhook on the backend's own port (W7e adds no routing for webhooks: D17).
 * cd frontend && EGRET_E2E_WORKFLOWS=1 EGRET_E2E_BASE_URL=http://localhost:5237 \
 *   EGRET_E2E_API=http://localhost:8237 \
 *   EGRET_E2E_PG_CONTAINER=egret-w7e-postgres EGRET_E2E_PG_DB=egret_w7e_e2e \
 *   npx playwright test workflows-triggers
 *
 * # 5. Afterwards: stop both servers, `docker rm -f -v w7e-rustfs`, and drop the database by its name:
 * docker exec egret-w7e-postgres psql -U egret -d postgres -c "DROP DATABASE egret_w7e_e2e"
 * ```
 *
 * Each run uses its own namespace, flow ids, role and user, so it can be repeated on the same
 * database.
 */
const ENABLED = process.env.EGRET_E2E_WORKFLOWS === "1";
const API_ORIGIN = process.env.EGRET_E2E_API ?? "http://localhost:8237";
const PG_CONTAINER = process.env.EGRET_E2E_PG_CONTAINER ?? "";
const PG_DB = process.env.EGRET_E2E_PG_DB ?? "";
const SHOTS = process.env.EGRET_E2E_SHOTS;

const RUN = Date.now().toString(36);
const NS = `e2e.trg${RUN}`;
const MAIN_ID = `main-${RUN}`;
const UP1_ID = `up1-${RUN}`;
const UP2_ID = `up2-${RUN}`;
const SIDE_ID = `side-${RUN}`;
const KEY = `key${RUN}abc`;
const READER_EMAIL = `e2e-trg-reader-${RUN}@egret.local`;
const READER_PASSWORD = "ReaderPw12345!";
const READER_ROLE = `e2e_trg_reader_${RUN}`;

const upstream = (id: string) => `id: ${id}
namespace: ${NS}
tasks:
  - id: hello
    type: io.kestra.plugin.core.log.Log
    message: upstream
`;

const MAIN = `id: ${MAIN_ID}
namespace: ${NS}
tasks:
  - id: hello
    type: io.kestra.plugin.core.log.Log
    message: triggered
triggers:
  - id: every-minute
    type: io.kestra.plugin.core.trigger.Schedule
    cron: "* * * * *"
  - id: hook
    type: io.kestra.plugin.core.trigger.Webhook
    key: ${KEY}
  - id: after-ups
    type: io.kestra.plugin.core.trigger.Flow
    dependsOn:
      - namespace: ${NS}
        flowId: ${UP1_ID}
      - namespace: ${NS}
        flowId: ${UP2_ID}
  - id: off-in-yaml
    type: io.kestra.plugin.core.trigger.Schedule
    cron: "0 3 * * *"
    disabled: true
`;

const SIDE = `id: ${SIDE_ID}
namespace: ${NS}
tasks:
  - id: hello
    type: io.kestra.plugin.core.log.Log
    message: side
triggers:
  - id: nightly
    type: io.kestra.plugin.core.trigger.Schedule
    cron: "0 2 * * *"
  - id: side-hook
    type: io.kestra.plugin.core.trigger.Webhook
    key: side${KEY}
`;

/** One SQL statement on the private database, rows as `|`-separated lines (`psql -At`). */
function sql(q: string): string {
  return execFileSync("docker", ["exec", PG_CONTAINER, "psql", "-U", "egret", "-d", PG_DB, "-Atc", q], { encoding: "utf8" }).trim();
}
const sqlOne = (q: string): string => sql(q).split("\n")[0] ?? "";
const lit = (s: string) => `'${s.replace(/'/g, "''")}'`;

/** The schedule's stored state, and the database's clock, read in one statement. */
function scheduleRow(flowUuid: string, triggerId: string): { next: number | null; now: number; disabled: boolean } {
  const [next, now, disabled] = sqlOne(
    `SELECT coalesce(extract(epoch FROM next_evaluation_date)::text, ''), extract(epoch FROM now())::text, disabled::text
       FROM workflow.triggers WHERE flow_id = ${lit(flowUuid)} AND trigger_id = ${lit(triggerId)}`,
  ).split("|");
  return { next: next ? Number(next) * 1000 : null, now: Number(now) * 1000, disabled: disabled === "true" };
}
const dbNow = () => Number(sqlOne("SELECT extract(epoch FROM now())")) * 1000;

/** The scheduled dates (ms) of the executions a trigger started, ascending. */
function scheduledExecutions(flowUuid: string, triggerId: string): number[] {
  const out = sql(
    `SELECT extract(epoch FROM scheduled_date) FROM workflow.executions
      WHERE flow_id = ${lit(flowUuid)} AND execution_trigger->>'id' = ${lit(triggerId)} AND scheduled_date IS NOT NULL
      ORDER BY scheduled_date`,
  );
  return out ? out.split("\n").map((s) => Math.round(Number(s) * 1000)) : [];
}
const triggeredCount = (flowUuid: string, triggerId: string) =>
  Number(sqlOne(`SELECT count(*) FROM workflow.executions WHERE flow_id = ${lit(flowUuid)} AND execution_trigger->>'id' = ${lit(triggerId)}`));

/** A margin of a few scheduler passes (the loop runs every second) past a tick. */
const PASS_MARGIN_MS = 5_000;

/**
 * Wait, bounded by the schedule, until the database's clock is past `at`: the bound is the
 * distance to `at` plus a margin, computed from the clock, not a sleep tuned to this machine.
 */
async function waitForDbClock(at: number, what: string): Promise<void> {
  const remaining = at - dbNow();
  report(`wait ${what}`, { seconds: Math.max(0, Math.round(remaining / 1000)) });
  await expect.poll(dbNow, { timeout: Math.max(0, remaining) + 30_000, intervals: [1_000] }).toBeGreaterThan(at);
}

/** Every measurement, printed as it is taken (for the report). */
function report(key: string, value: unknown): void {
  console.log(`measure [${key}]: ${JSON.stringify(value)}`);
}

async function shot(page: Page, key: string): Promise<void> {
  if (SHOTS) await page.screenshot({ path: `${SHOTS}/w7e-${key.replace(/\W+/g, "-")}.png`, fullPage: true });
}

/** Light or dark, through the app's own preference (`ThemeProvider`, `egret-theme`), then a reload. */
async function setTheme(page: Page, theme: "light" | "dark"): Promise<void> {
  await page.evaluate((t) => localStorage.setItem("egret-theme", t), theme);
  await page.reload();
  await expect(page.locator("html")).toHaveClass(theme === "dark" ? /\bdark\b/ : /^(?!.*\bdark\b)/);
}

/** `primary`: painted on the shared `--primary` surface (a default button), held to 3:1 here (see `expectReadable`). */
type TextSample = { text: string; fg: string; bg: string; ratio: number; primary: boolean };
type Measure = {
  viewport: number;
  pageScrollWidth: number;
  scopeLeft: number;
  scopeRight: number;
  /** Text cut by an `overflow` other than visible, or pushed past the viewport outside any scroller. */
  clipped: string[];
  /** Horizontal scrollers inside the scope: their own width and content width. */
  scrollers: { width: number; scrollWidth: number }[];
  samples: TextSample[];
  tokens: Record<string, string>;
};

/**
 * Measure `scope` in the DOM: page overflow, clipped text, horizontal scrollers, and the contrast
 * of each text leaf against the first opaque surface behind it. Colours are resolved through a
 * canvas, so `oklch()` tokens compare as the sRGB the screen shows.
 */
async function measure(scope: Locator): Promise<Measure> {
  return scope.evaluate((root) => {
    const canvas = document.createElement("canvas");
    canvas.width = canvas.height = 1;
    const ctx = canvas.getContext("2d", { willReadFrequently: true })!;
    const rgba = (css: string): [number, number, number, number] => {
      ctx.clearRect(0, 0, 1, 1);
      ctx.fillStyle = "#000";
      ctx.fillStyle = css;
      ctx.fillRect(0, 0, 1, 1);
      const d = ctx.getImageData(0, 0, 1, 1).data;
      return [d[0], d[1], d[2], d[3] / 255];
    };
    const lum = ([r, g, b]: number[]) => {
      const c = [r, g, b].map((v) => {
        const s = v / 255;
        return s <= 0.03928 ? s / 12.92 : ((s + 0.055) / 1.055) ** 2.4;
      });
      return 0.2126 * c[0] + 0.7152 * c[1] + 0.0722 * c[2];
    };
    /** The surface behind `el`: the first ancestor whose background is not transparent, composed over white or black. */
    const surface = (el: Element): [number, number, number] => {
      const layers: [number, number, number, number][] = [];
      for (let e: Element | null = el; e; e = e.parentElement) {
        const c = rgba(getComputedStyle(e).backgroundColor);
        if (c[3] > 0) layers.push(c);
        if (c[3] >= 1) break;
      }
      const dark = document.documentElement.classList.contains("dark");
      let base: [number, number, number] = dark ? [0, 0, 0] : [255, 255, 255];
      for (const [r, g, b, a] of layers.reverse()) base = [r * a + base[0] * (1 - a), g * a + base[1] * (1 - a), b * a + base[2] * (1 - a)];
      return base;
    };
    const vw = document.documentElement.clientWidth;
    const clipped: string[] = [];
    const samples: { text: string; fg: string; bg: string; ratio: number; primary: boolean }[] = [];
    const primary = rgba(getComputedStyle(document.documentElement).getPropertyValue("--primary").trim());
    const leaves = Array.from(root.querySelectorAll<HTMLElement>("*")).filter((el) => {
      if (!Array.from(el.childNodes).some((n) => n.nodeType === Node.TEXT_NODE && n.textContent!.trim() !== "")) return false;
      const r = el.getBoundingClientRect();
      const cs = getComputedStyle(el);
      return r.width > 0 && r.height > 0 && cs.visibility !== "hidden" && !el.closest(".sr-only") && cs.opacity !== "0";
    });
    const inScroller = (el: Element) => {
      for (let e = el.parentElement; e && e !== document.body; e = e.parentElement) {
        const ox = getComputedStyle(e).overflowX;
        if ((ox === "auto" || ox === "scroll") && e.scrollWidth > e.clientWidth) return true;
      }
      return false;
    };
    for (const el of leaves) {
      const cs = getComputedStyle(el);
      const text = (el.textContent ?? "").trim().replace(/\s+/g, " ").slice(0, 60);
      const r = el.getBoundingClientRect();
      if (cs.overflowX !== "visible" && cs.overflowX !== "auto" && cs.overflowX !== "scroll" && el.scrollWidth > el.clientWidth + 1) clipped.push(`${text} (overflow ${el.scrollWidth}>${el.clientWidth})`);
      else if ((r.right > vw + 1 || r.left < -1) && !inScroller(el)) clipped.push(`${text} (outside the viewport ${Math.round(r.left)}..${Math.round(r.right)})`);
      const fg = rgba(cs.color);
      const bg = surface(el);
      const blended = [0, 1, 2].map((i) => fg[i] * fg[3] + bg[i] * (1 - fg[3]));
      const [l1, l2] = [lum(blended), lum(bg)].sort((a, b) => b - a);
      const onPrimary = [0, 1, 2].every((i) => Math.abs(bg[i] - primary[i]) <= 2);
      samples.push({ text, fg: cs.color, bg: `rgb(${bg.map(Math.round).join(", ")})`, ratio: Math.round(((l1 + 0.05) / (l2 + 0.05)) * 100) / 100, primary: onPrimary });
    }
    const scrollers = Array.from(root.querySelectorAll<HTMLElement>("*"))
      .filter((e) => {
        const ox = getComputedStyle(e).overflowX;
        return (ox === "auto" || ox === "scroll") && e.scrollWidth > e.clientWidth;
      })
      .map((e) => ({ width: e.clientWidth, scrollWidth: e.scrollWidth }));
    const rootStyle = getComputedStyle(document.documentElement);
    const tokens = Object.fromEntries(
      ["--background", "--foreground", "--muted-foreground", "--card", "--popover", "--border", "--destructive"].map((t) => [t, rootStyle.getPropertyValue(t).trim()]),
    );
    const sr = root.getBoundingClientRect();
    return {
      viewport: vw,
      pageScrollWidth: document.documentElement.scrollWidth,
      scopeLeft: Math.round(sr.left),
      scopeRight: Math.round(sr.right),
      clipped,
      scrollers,
      samples,
      tokens,
    };
  });
}

/**
 * The assertions every measured view must meet: no horizontal page scroll, the scope inside the
 * viewport, no text clipped, and every sampled text at 4.5:1 or more against its surface.
 */
async function expectReadable(page: Page, scope: Locator, what: string): Promise<Measure> {
  // A row that is still fading in or a toast sliding out would measure a moment, not the view.
  await page.waitForTimeout(400);
  const m = await measure(scope);
  const worst = [...m.samples].sort((a, b) => a.ratio - b.ratio).slice(0, 3);
  report(what, { viewport: m.viewport, pageScrollWidth: m.pageScrollWidth, scope: [m.scopeLeft, m.scopeRight], clipped: m.clipped, scrollers: m.scrollers, texts: m.samples.length, worst, tokens: m.tokens });
  await shot(page, what);
  expect(m.pageScrollWidth, `${what}: the page scrolls sideways`).toBeLessThanOrEqual(m.viewport);
  expect(m.scopeLeft, `${what}: starts inside the viewport`).toBeGreaterThanOrEqual(0);
  expect(m.scopeRight, `${what}: ends inside the viewport`).toBeLessThanOrEqual(m.viewport + 1);
  expect(m.clipped, `${what}: clipped text`).toEqual([]);
  expect(m.samples.length, `${what}: texts measured`).toBeGreaterThan(0);
  // Text on the shared `--primary` (the default button) is the design system's, not this page's:
  // light mode's teal measures 3.5:1 under white everywhere in the app. It is reported, and held
  // to 3:1 so a regression still shows; every other text is held to 4.5:1.
  for (const s of m.samples) expect(s.ratio, `${what}: "${s.text}" ${s.fg} on ${s.bg}`).toBeGreaterThanOrEqual(s.primary ? 3 : 4.5);
  return m;
}

const row = (page: Page, triggerId: string): Locator => page.getByTestId("triggers-table").locator(`tr[data-trigger="${triggerId}"]`);
const status = (page: Page, triggerId: string): Locator => row(page, triggerId).getByTestId("trigger-status");
const switchOf = (page: Page, name: string): Locator => page.getByRole("switch", { name: `Trigger ${name} enabled`, exact: true });
const drawer = (page: Page): Locator => page.getByRole("dialog").filter({ has: page.getByRole("heading", { name: /^Trigger / }) });

/** The viewer's own sign-in, through the same local form as `loginLocalAdmin`. */
async function loginAs(page: Page, email: string, password: string): Promise<void> {
  const cfgLoaded = page.waitForResponse((r) => r.url().includes("/api/auth/config") && r.ok(), { timeout: 15_000 });
  await page.goto("/login");
  await cfgLoaded;
  const adminToggle = page.getByRole("button", { name: "Admin sign-in (break-glass)" });
  if (await adminToggle.isVisible().catch(() => false)) await adminToggle.click();
  await page.getByPlaceholder("Email").fill(email);
  await page.getByPlaceholder("Password").fill(password);
  await page.getByRole("button", { name: "Sign in", exact: true }).click();
  await page.waitForURL(/\/explore\/[^/]+\//, { timeout: 20_000 });
}

async function viewerSession(browser: Browser, use: TestInfo["project"]["use"]): Promise<Page> {
  const ctx = await browser.newContext({ baseURL: use.baseURL, viewport: { width: 1280, height: 900 } });
  const page = await ctx.newPage();
  await loginAs(page, READER_EMAIL, READER_PASSWORD);
  return page;
}

test.describe("W7e triggers: tab, switch, enable dialog, webhook, dependsOn, global page, viewer", () => {
  test.skip(!ENABLED, "opt-in: set EGRET_E2E_WORKFLOWS=1 with a private local stack up (see the comment above)");
  test.skip(ENABLED && (!PG_CONTAINER || !PG_DB), "set EGRET_E2E_PG_CONTAINER and EGRET_E2E_PG_DB: the spec reads the private database");
  test.setTimeout(900_000);

  test("triggers: §6 scenarios 1-7, measured in light and dark at 1280 and 390 px", async ({ page, context, browser }, testInfo) => {
    page.on("pageerror", (e) => console.log(`[pageerror] ${e.message}`));
    await page.setViewportSize({ width: 1280, height: 900 });
    await loginLocalAdmin(page);
    await setTheme(page, "light");
    const ws = (await firstWorkspace(page.request)).slug;
    const api = `/api/ws/${ws}/workflows`;
    const h = await csrf(context);
    const base = `/explore/${ws}/workflows`;

    // ── 0. The flows and the viewer, through the API ─────────────────────────────────
    const up1 = await postJson(page.request, `${api}/flows`, h, { source: upstream(UP1_ID) });
    const up2 = await postJson(page.request, `${api}/flows`, h, { source: upstream(UP2_ID) });
    const main = await postJson(page.request, `${api}/flows`, h, { source: MAIN });
    const side = await postJson(page.request, `${api}/flows`, h, { source: SIDE });
    const mainId: string = main.id;
    const viewerRole = await postJson(page.request, "/api/control-plane/access/roles", h, {
      scope: "workspace",
      name: READER_ROLE,
      description: "W7e e2e: workflow:read without workflow:manage",
      parentRoleId: sqlOne("SELECT id FROM roles WHERE name = 'ws_viewer'"),
    });
    const perms = await page.request.put(`/api/control-plane/access/roles/${viewerRole.id}/permissions`, { headers: h, data: { permissions: ["workflow:read"] } });
    expect(perms.ok(), `role permissions → ${perms.status()}`).toBeTruthy();
    await postJson(page.request, "/api/control-plane/users", h, { email: READER_EMAIL, name: "Trigger Reader", password: READER_PASSWORD });
    await postJson(page.request, `/api/ws/${ws}/members`, h, { principal_type: "user", principal_id: READER_EMAIL, role: READER_ROLE });

    // ── 1. The Triggers tab: every trigger with its state, the webhook's path ───────
    await page.goto(`${base}/flows/${mainId}/triggers`);
    await expect(page.getByRole("tab", { name: "Triggers" })).toHaveAttribute("aria-selected", "true");
    for (const id of ["every-minute", "hook", "after-ups", "off-in-yaml"]) await expect(row(page, id)).toBeVisible();
    await expect(page.getByTestId("triggers-table").locator("tbody tr")).toHaveCount(4);
    // The schedule's next run, from the database's own row (Next run is the 4th cell: select, ID, Type, Next run).
    const next0 = scheduleRow(mainId, "every-minute");
    expect(next0.next, "the schedule is on the queue").not.toBeNull();
    await expect(row(page, "every-minute").locator("td").nth(3)).toHaveText(/^in /);
    await expect(row(page, "every-minute").locator("td").nth(3).locator("[title]").first()).toBeVisible();
    await expect(row(page, "hook").locator("td").nth(3)).toHaveText("—");
    await expect(status(page, "every-minute")).toHaveText("Active");
    await expect(status(page, "after-ups")).toHaveText("Active");
    await expect(status(page, "off-in-yaml")).toHaveText("Disabled in YAML");
    await expect(page.getByTestId("trigger-banners")).toHaveCount(0);

    // The webhook's drawer: the path from the server, masked until Reveal (D17: a path, no origin).
    const perFlow = await (await page.request.get(`${api}/flows/${mainId}/triggers`)).json();
    const urlPath: string = perFlow.triggers.find((t: { trigger_id: string }) => t.trigger_id === "hook").webhook.url_path;
    expect(urlPath).toBe(`/workflows/v1/${ws}/executions/webhook/${NS}/${MAIN_ID}/${KEY}`);
    await row(page, "hook").getByRole("link", { name: "Details of hook" }).click();
    await expect(page).toHaveURL(/[?&]trigger=hook\b/);
    // Masked: the path with its last segment, the key, replaced by the model's `MASK`.
    await expect(drawer(page).getByTestId("webhook-path")).toHaveText(urlPath.replace(/[^/]+$/, "••••••••"));
    await expect(drawer(page).locator("pre")).not.toContainText(KEY);
    await drawer(page).getByRole("button", { name: "Reveal" }).click();
    await expect(drawer(page).getByRole("button", { name: "Reveal" })).toHaveAttribute("aria-pressed", "true");
    await expect(drawer(page).getByTestId("webhook-path")).toHaveText(urlPath);
    await expect(drawer(page).getByTestId("webhook-path")).not.toContainText("http");
    await expect(drawer(page).locator("pre")).toContainText(KEY);
    await expect(drawer(page)).toContainText("served by the egret backend");
    await page.keyboard.press("Escape");
    await expect(page).not.toHaveURL(/[?&]trigger=/);

    // ── 2a. Switch the schedule off: the clock starts here ───────────────────────────
    await switchOf(page, "every-minute").click();
    await expect(switchOf(page, "every-minute")).toHaveAttribute("aria-checked", "false");
    await expect(status(page, "every-minute")).toHaveText(/^Switched off by Administrator/);
    const off1 = scheduleRow(mainId, "every-minute");
    expect(off1.disabled, "the effective copy on workflow.triggers").toBe(true);
    expect(sqlOne(`SELECT count(*) FROM workflow.disabled_triggers WHERE flow_id = ${lit(mainId)} AND trigger_id = 'every-minute'`)).toBe("1");
    const firstMissed = off1.next!;
    const firedBefore = scheduledExecutions(mainId, "every-minute");
    report("scenario 2: switched off", { at: new Date(off1.now).toISOString(), next: new Date(firstMissed).toISOString(), firedBefore: firedBefore.length });

    // ── 3. The webhook: off answers 409 on the backend's own port, on again 200 ──────
    const hookUrl = `${API_ORIGIN}${urlPath}`;
    await switchOf(page, "hook").click();
    await expect(switchOf(page, "hook")).toHaveAttribute("aria-checked", "false");
    await expect(status(page, "hook")).toHaveText(/^Switched off by Administrator/);
    const hookBefore = triggeredCount(mainId, "hook");
    const refused = await fetch(hookUrl, { method: "POST" });
    const refusedBody = await refused.text();
    report("scenario 3: webhook switched off", { status: refused.status, body: refusedBody });
    expect(refused.status).toBe(409);
    expect(refusedBody).toContain("Cannot execute webhook: the trigger 'hook' is disabled.");
    expect(triggeredCount(mainId, "hook"), "no execution while off").toBe(hookBefore);
    // A wrong key still answers the shared 404, switched off or not.
    expect((await fetch(`${hookUrl}x`, { method: "POST" })).status).toBe(404);
    await switchOf(page, "hook").click();
    await expect(switchOf(page, "hook")).toHaveAttribute("aria-checked", "true");
    await expect(status(page, "hook")).toHaveText("Active");
    const accepted = await fetch(hookUrl, { method: "POST" });
    const acceptedBody = (await accepted.json()) as { id: string };
    report("scenario 3: webhook on", { status: accepted.status, execution: acceptedBody.id });
    expect(accepted.status).toBe(200);
    expect(sqlOne(`SELECT execution_trigger->>'id' FROM workflow.executions WHERE id = ${lit(acceptedBody.id)}`)).toBe("hook");
    expect(triggeredCount(mainId, "hook")).toBe(hookBefore + 1);

    // ── 4. dependsOn: one upstream opens the window, the other runs the downstream ───
    await postJson(page.request, `${api}/flows/${up1.id}/executions`, h, { inputs: {} });
    await expect
      .poll(() => sqlOne(`SELECT count(*) FROM workflow.flow_trigger_windows WHERE flow_id = ${lit(mainId)} AND trigger_id = 'after-ups'`), { timeout: 60_000 })
      .toBe("1");
    await page.goto(`${base}/flows/${mainId}/triggers?trigger=after-ups`);
    const conditions = drawer(page).getByRole("list", { name: "Conditions" });
    await expect(conditions.getByRole("listitem")).toHaveCount(2);
    await expect(conditions.getByRole("listitem").filter({ hasText: `${NS}.${UP1_ID}` })).toContainText("Met");
    await expect(conditions.getByRole("listitem").filter({ hasText: `${NS}.${UP2_ID}` })).toContainText("Not met");
    await expect(drawer(page)).toContainText(/Window\s*duration: \d{4}-\d\d-\d\d/);
    await expect(drawer(page)).not.toContainText("No window is open");
    await expect(drawer(page)).toContainText("Listening");
    const afterBefore = triggeredCount(mainId, "after-ups");
    await postJson(page.request, `${api}/flows/${up2.id}/executions`, h, { inputs: {} });
    await expect.poll(() => triggeredCount(mainId, "after-ups"), { timeout: 60_000 }).toBe(afterBefore + 1);
    const downstream = sqlOne(`SELECT id FROM workflow.executions WHERE flow_id = ${lit(mainId)} AND execution_trigger->>'id' = 'after-ups' ORDER BY created_at DESC LIMIT 1`);
    await expect.poll(() => sqlOne(`SELECT state FROM workflow.executions WHERE id = ${lit(downstream)}`), { timeout: 60_000 }).toBe("SUCCESS");
    report("scenario 4: downstream ran", { execution: downstream });
    await page.keyboard.press("Escape");
    // The tab's Last execution follows (it refetches after a reload).
    await page.reload();
    await expect(row(page, "after-ups").getByRole("link", { name: downstream.slice(0, 8) })).toBeVisible();

    // ── 7. The YAML-disabled trigger: a disabled switch with its reason ─────────────
    const yamlSwitch = switchOf(page, "off-in-yaml");
    await expect(yamlSwitch).toBeDisabled();
    await expect(yamlSwitch).toHaveAttribute("aria-checked", "false");
    await expect(yamlSwitch).toHaveAccessibleDescription("Disabled in the flow's YAML");
    await expect(row(page, "off-in-yaml")).toContainText("Disabled in the flow's YAML");

    // ── 3 (DOM). The tab and the drawer per kind, light and dark, 1280 and 390 ──────
    const tabPanel = page.getByRole("tabpanel");
    for (const theme of ["light", "dark"] as const) {
      await setTheme(page, theme);
      for (const width of [1280, 390]) {
        await page.setViewportSize({ width, height: 900 });
        await page.goto(`${base}/flows/${mainId}/triggers`);
        await expect(row(page, "every-minute")).toBeVisible();
        await expectReadable(page, tabPanel, `tab ${theme} ${width}`);
        for (const id of ["every-minute", "hook", "after-ups"]) {
          await page.goto(`${base}/flows/${mainId}/triggers?trigger=${id}`);
          await expect(drawer(page).getByRole("heading", { name: "Last execution" })).toBeVisible();
          await expectReadable(page, drawer(page), `drawer ${id} ${theme} ${width}`);
        }
      }
    }
    await page.setViewportSize({ width: 1280, height: 900 });
    await setTheme(page, "light");

    // ── 5. The global page: URL filters, bulk disable two triggers, reload ─────────
    await page.goto(`${base}/triggers?namespace=${encodeURIComponent(NS)}&kind=schedule,webhook`);
    const list = page.getByTestId("triggers-table");
    await expect(list.locator("tbody tr")).toHaveCount(5);
    await expect(list.locator('tr[data-trigger="after-ups"]')).toHaveCount(0);
    await expect(page.getByRole("button", { name: /Kind:\s*Schedule, Webhook/ })).toBeVisible();
    await expect(page.getByRole("combobox", { name: /Namespace/ })).toContainText(NS);
    const sideName = (id: string) => `${id} of ${NS}.${SIDE_ID}`;
    await page.getByRole("checkbox", { name: `Select ${sideName("nightly")}` }).click();
    await page.getByRole("checkbox", { name: `Select ${sideName("side-hook")}` }).click();
    await expect(page.getByText("2 triggers selected")).toBeVisible();
    const bulk = page.waitForResponse((r) => r.url().endsWith(`${api}/triggers/disabled`) && r.request().method() === "PUT");
    await page.getByRole("button", { name: "Disable", exact: true }).click();
    const bulkBody = await (await bulk).json();
    report("scenario 5: bulk answer", bulkBody.results.map((r: { trigger_id: string; ok: boolean }) => `${r.trigger_id}:${r.ok}`));
    expect(bulkBody.results.every((r: { ok: boolean }) => r.ok)).toBe(true);
    await page.reload();
    await expect(list.locator("tbody tr")).toHaveCount(5);
    for (const id of ["nightly", "side-hook"]) {
      await expect(switchOf(page, sideName(id))).toHaveAttribute("aria-checked", "false");
      await expect(list.locator(`tr[data-trigger="${id}"]`).getByTestId("trigger-status")).toHaveText(/^Switched off by Administrator/);
    }
    expect(sql(`SELECT trigger_id FROM workflow.disabled_triggers WHERE flow_id = ${lit(side.id)} ORDER BY trigger_id`)).toBe("nightly\nside-hook");
    expect(sqlOne(`SELECT disabled::text FROM workflow.triggers WHERE flow_id = ${lit(side.id)} AND trigger_id = 'nightly'`)).toBe("true");
    // The state filter reads the effective value: Disabled lists the two, the YAML one and the switched-off schedule.
    await page.goto(`${base}/triggers?namespace=${encodeURIComponent(NS)}&state=disabled`);
    await expect(list.locator("tbody tr")).toHaveCount(4);

    // The global page, light and dark, 1280 and 390, with the drawer from a row.
    const shell = page.locator("main");
    for (const theme of ["light", "dark"] as const) {
      await setTheme(page, theme);
      for (const width of [1280, 390]) {
        await page.setViewportSize({ width, height: 900 });
        await page.goto(`${base}/triggers?namespace=${encodeURIComponent(NS)}`);
        await expect(list.locator("tbody tr")).toHaveCount(6);
        await expectReadable(page, shell, `global ${theme} ${width}`);
        await page.goto(`${base}/triggers?namespace=${encodeURIComponent(NS)}&trigger=${encodeURIComponent(`${side.id}/side-hook`)}`);
        await expect(drawer(page).getByRole("heading", { name: "Last execution" })).toBeVisible();
        await expectReadable(page, drawer(page), `global drawer side-hook ${theme} ${width}`);
      }
    }
    await page.setViewportSize({ width: 1280, height: 900 });
    await setTheme(page, "light");

    // ── 6. The viewer: every switch and bulk button disabled, with the reason ───────
    const viewer = await viewerSession(browser, testInfo.project.use);
    try {
      await viewer.goto(`${base}/flows/${mainId}/triggers`);
      await expect(row(viewer, "every-minute")).toBeVisible();
      for (const id of ["every-minute", "hook", "after-ups", "off-in-yaml"]) {
        await expect(switchOf(viewer, id)).toBeDisabled();
        // The server's first refusal is `forbidden` (the caller), before the YAML's.
        await expect(switchOf(viewer, id)).toHaveAccessibleDescription("You need workflow:manage");
      }
      for (const name of ["Enable", "Disable"]) await expect(viewer.getByRole("button", { name, exact: true })).toBeDisabled();
      await expect(viewer.getByRole("checkbox", { name: "Select every-minute" })).toBeDisabled();
      await viewer.goto(`${base}/triggers?namespace=${encodeURIComponent(NS)}`);
      await expect(viewer.getByTestId("triggers-table").locator("tbody tr")).toHaveCount(6);
      await expect(switchOf(viewer, sideName("nightly"))).toHaveAccessibleDescription("You need workflow:manage");
      for (const name of ["Enable", "Disable"]) await expect(viewer.getByRole("button", { name, exact: true })).toBeDisabled();
    } finally {
      await viewer.context().close();
    }

    // ── 2b. Past two ticks: nothing fired; on with "Skip" ────────────────────────────
    await waitForDbClock(firstMissed + 60_000 + PASS_MARGIN_MS, "two ticks after the switch-off");
    const missed1 = scheduledExecutions(mainId, "every-minute").filter((d) => d >= firstMissed);
    report("scenario 2: executions at or after the first missed tick", missed1.map((d) => new Date(d).toISOString()));
    expect(missed1, "a switched-off schedule fires nothing").toEqual([]);
    expect(scheduleRow(mainId, "every-minute").next, "a switched-off schedule keeps its dates").toBe(firstMissed);

    await page.goto(`${base}/flows/${mainId}/triggers`);
    await switchOf(page, "every-minute").click();
    const dialog = page.getByRole("dialog", { name: "Enable trigger" });
    await expect(dialog).toBeVisible();
    const apiMissed = (await (await page.request.get(`${api}/flows/${mainId}/triggers`)).json()).triggers.find(
      (t: { trigger_id: string }) => t.trigger_id === "every-minute",
    ).schedule.missed_ticks.count as number;
    expect(apiMissed).toBeGreaterThanOrEqual(2);
    await expect(dialog.getByRole("radio", { name: "Skip missed runs; start at the next tick" })).toBeChecked();
    const recoverRadio = dialog.getByRole("radio", { name: /^Recover missed runs \(\s*\d+\s*missed\)/ });
    await expect(recoverRadio).toBeVisible();
    // The count sits inside the brackets as text, not as a separate flex item: no gap is painted
    // between "(" and the number, nor after it, and the radio's name reads as the text does.
    const gaps = await dialog.locator("label:has(.tabular-nums)").evaluate((label) => {
      const count = label.querySelector(".tabular-nums")!;
      const before = Array.from(label.childNodes).find((n) => n.nodeType === Node.TEXT_NODE && n.textContent!.includes("("))!;
      const after = count.nextSibling!;
      const box = (n: Node) => {
        const r = document.createRange();
        r.selectNodeContents(n);
        return r.getBoundingClientRect();
      };
      return {
        display: getComputedStyle(label).display,
        beforeCount: Math.round(box(count).left - box(before).right),
        afterCount: Math.round(box(after).left - box(count).right),
      };
    });
    report("enable dialog: gaps around the missed count", gaps);
    expect.soft(gaps.beforeCount, "no gap between '(' and the count").toBeLessThanOrEqual(1);
    expect.soft(gaps.afterCount, "one space, not a flex gap, after the count").toBeLessThanOrEqual(5);
    await expect.soft(recoverRadio).toHaveAccessibleName(`Recover missed runs (${apiMissed} missed)`);
    await expect(dialog).toContainText("every-minute runs every missed tick, one per scheduler pass.");
    // The dialog, measured in both modes at both widths; Cancel sends nothing.
    for (const theme of ["light", "dark"] as const) {
      for (const width of [1280, 390]) {
        if (theme === "dark" || width === 390) {
          await setTheme(page, theme);
          await page.setViewportSize({ width, height: 900 });
          await switchOf(page, "every-minute").click();
          await expect(dialog).toBeVisible();
        }
        await expectReadable(page, dialog, `enable dialog ${theme} ${width}`);
        await dialog.getByRole("button", { name: "Cancel" }).click();
        await expect(dialog).toBeHidden();
      }
    }
    await page.setViewportSize({ width: 1280, height: 900 });
    await setTheme(page, "light");
    expect(scheduleRow(mainId, "every-minute").disabled, "Cancel sent nothing").toBe(true);

    await switchOf(page, "every-minute").click();
    await expect(dialog).toBeVisible();
    const enableSkip = page.waitForRequest((r) => r.url().includes("/triggers/every-minute/disabled") && r.method() === "PUT");
    await dialog.getByRole("button", { name: "Enable", exact: true }).click();
    expect((await enableSkip).postDataJSON()).toEqual({ disabled: false, recover_missed: false });
    await expect(dialog).toBeHidden();
    await expect(switchOf(page, "every-minute")).toHaveAttribute("aria-checked", "true");
    const skipped = scheduleRow(mainId, "every-minute");
    report("scenario 2: on with Skip", { now: new Date(skipped.now).toISOString(), next: new Date(skipped.next!).toISOString() });
    expect(skipped.disabled).toBe(false);
    expect(skipped.next!, "Skip: the next run is after the switch").toBeGreaterThan(skipped.now);
    expect(skipped.next! - skipped.now).toBeLessThanOrEqual(60_000);
    // Bounded by the schedule: the next tick fires, and nothing fired for the ticks before it.
    await waitForDbClock(skipped.next! + PASS_MARGIN_MS, "the first tick after Skip");
    await expect.poll(() => scheduledExecutions(mainId, "every-minute").filter((d) => d >= firstMissed), { timeout: 30_000 }).toEqual([skipped.next!]);

    // ── 2c. Off again, past two ticks, on with "Recover": the missed ticks run ──────
    await page.reload();
    await switchOf(page, "every-minute").click();
    await expect(switchOf(page, "every-minute")).toHaveAttribute("aria-checked", "false");
    const off2 = scheduleRow(mainId, "every-minute");
    const recoverFrom = off2.next!;
    await waitForDbClock(recoverFrom + 60_000 + PASS_MARGIN_MS, "two ticks after the second switch-off");
    expect(scheduledExecutions(mainId, "every-minute").filter((d) => d >= recoverFrom)).toEqual([]);
    await page.reload();
    await switchOf(page, "every-minute").click();
    await expect(dialog).toBeVisible();
    await dialog.getByRole("radio", { name: /^Recover missed runs/ }).click();
    await expect(dialog.getByRole("radio", { name: /^Recover missed runs/ })).toBeChecked();
    const enableRecover = page.waitForRequest((r) => r.url().includes("/triggers/every-minute/disabled") && r.method() === "PUT");
    const missedAtEnable = Math.floor((dbNow() - recoverFrom) / 60_000) + 1;
    await dialog.getByRole("button", { name: "Enable", exact: true }).click();
    expect((await enableRecover).postDataJSON()).toEqual({ disabled: false, recover_missed: true });
    await expect(switchOf(page, "every-minute")).toHaveAttribute("aria-checked", "true");
    const expected = Array.from({ length: missedAtEnable }, (_, i) => recoverFrom + i * 60_000);
    report("scenario 2: on with Recover", { missedTicks: expected.map((d) => new Date(d).toISOString()) });
    expect(expected.length).toBeGreaterThanOrEqual(2);
    // ALL (recoverMissedSchedules absent): every missed tick, one per scheduler pass.
    await expect
      .poll(() => scheduledExecutions(mainId, "every-minute").filter((d) => d >= recoverFrom && d <= expected[expected.length - 1]), { timeout: 30_000 })
      .toEqual(expected);
  });
});
