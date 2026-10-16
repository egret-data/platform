import { execFileSync } from "node:child_process";
import { test, expect, type Browser, type Locator, type Page } from "@playwright/test";
import { csrf, firstWorkspace, loginLocalAdmin, postJson } from "./helpers";

/**
 * W7f (schedule backfill) end to end, spec
 * `docs/en/specs/2026-10-15-workflow-w7f-backfill-design.md` §6 "Playwright", scenarios 1-7:
 *
 * 1. a flow with a `* * * * *` schedule, a flow label `team: data` and an input. Backfill the
 *    last five minutes with an input and the labels `team: ops`, `reason: test`: five executions
 *    appear, the strip goes away, each execution's overview shows `team: ops` and "Backfill";
 *    light and dark mode;
 * 2. re-run: a window the schedule already ran creates a second execution per tick;
 * 3. a seconds cron (`withSeconds`) and a one-hour window: pause, no new executions; resume, they
 *    continue; delete with the confirm: the strip goes and the next run is the regular one;
 * 4. the form: future dates cannot be picked; Start after End disables the submit with its reason;
 *    a missing required input; a second create through the API answers 409;
 * 5. the global page: filter `backfill=running`, bulk pause two, reload, both paused; bulk delete;
 * 6. a viewer without `workflow:manage`: the Backfill button and the strip's actions are disabled
 *    with the reason;
 * 7. the trigger switched off during a backfill: "Frozen while the trigger is off"; on again: it
 *    resumes.
 *
 * And §6's Playwright part is followed by a DOM measurement (`measure [...]` lines) in light and
 * dark mode at 1280 and 390 px: the Triggers tab's column and strip, the drawer's Backfill section,
 * the create dialog, and the global page.
 *
 * Every wait on the scheduler is bounded by the schedule or by the database's clock (`now()`),
 * never a fixed sleep tuned to this machine. Scenario 3's and 4's second create go through the
 * backend's own port (`API_ORIGIN`): W7f adds no routing for the API, and the session cookie is the
 * same one the page uses. Scenario 2 needs two regular ticks to have fired, so it waits for them.
 *
 * Opt-in (`EGRET_E2E_WORKFLOWS=1`). The sign-in limit (5 per 5 minutes) is raised on the PRIVATE
 * backend only (`EGRET_RATE_LIMIT__LOGIN_MAX_ATTEMPTS=200`), since every test signs in.
 *
 * HOW TO RUN IT (a private stack, never the shared dev stack: no Postgres :5432, no shared
 * RustFS/MinIO bucket, no kind cluster, no Valkey :6399). Names used here: database
 * `egret_w7f_e2e` on the branch's Postgres container `egret-w7f-postgres` (:5449), RustFS
 * `w7f-rustfs` (:9495), backend :8247, Vite :5247.
 *
 * ```bash
 * # 1. Database and private storage (from the egret worktree root)
 * docker exec egret-w7f-postgres psql -U egret -d postgres -c 'CREATE DATABASE egret_w7f_e2e'
 * docker run -d --name w7f-rustfs -p 9495:9000 -e RUSTFS_ACCESS_KEY=egret -e RUSTFS_SECRET_KEY=egretsecret rustfs/rustfs:1.0.1
 * curl --aws-sigv4 "aws:amz:us-east-1:s3" --user egret:egretsecret -X PUT http://localhost:9495/warehouse
 *
 * # 2. Backend (env as in workflows-triggers.spec.ts, with the W7f ports and the raised login limit)
 * export DATABASE_URL=postgres://egret:egret@localhost:5449/egret_w7f_e2e
 * export EGRET_DATABASE__URL=$DATABASE_URL EGRET_SERVER__PORT=8247 EGRET_SERVER__HOST=127.0.0.1
 * export EGRET_OIDC__ENABLED=false EGRET_WORKFLOW__WORKER_ENABLED=true
 * export EGRET_WORKFLOW__SCHEDULER_ENABLED=true EGRET_WORKFLOW__WEBHOOKS_ENABLED=true
 * export EGRET_STORAGE__ENDPOINT=http://localhost:9495 EGRET_STORAGE__BUCKET=warehouse
 * export EGRET_STORAGE__ACCESS_KEY=egret EGRET_STORAGE__SECRET_KEY=egretsecret
 * export EGRET_ADMIN_PASSWORD='Admin12345!' EGRET_RATE_LIMIT__LOGIN_MAX_ATTEMPTS=200
 * ./target-check/debug/egret migrate && ./target-check/debug/egret init
 * ./target-check/debug/egret serve                                  # in one shell
 *
 * # 3. Frontend (the committed config; Vite proxies /api to :8247)
 * cd frontend && EGRET_E2E_VITE_PORT=5247 EGRET_E2E_API=http://localhost:8247 \
 *   npx vite --config tests/e2e/vite.e2e.config.mts                  # in another shell
 *
 * # 4. The spec
 * cd frontend && EGRET_E2E_WORKFLOWS=1 EGRET_E2E_BASE_URL=http://localhost:5247 \
 *   EGRET_E2E_API=http://localhost:8247 \
 *   EGRET_E2E_PG_CONTAINER=egret-w7f-postgres EGRET_E2E_PG_DB=egret_w7f_e2e \
 *   npx playwright test workflows-backfill
 *
 * # 5. Teardown: stop both servers, `docker rm -f -v w7f-rustfs`, then
 * docker exec egret-w7f-postgres psql -U egret -d postgres -c 'DROP DATABASE egret_w7f_e2e'
 * ```
 *
 * Each run uses its own namespace and flow ids, so it can be repeated on the same database.
 */
const ENABLED = process.env.EGRET_E2E_WORKFLOWS === "1";
const API_ORIGIN = process.env.EGRET_E2E_API ?? "http://localhost:8247";
const PG_CONTAINER = process.env.EGRET_E2E_PG_CONTAINER ?? "egret-w7f-postgres";
const PG_DB = process.env.EGRET_E2E_PG_DB ?? "egret_w7f_e2e";
const SHOTS = process.env.EGRET_E2E_SHOTS;

const RUN = Date.now().toString(36);
const NS = `e2e.w7f${RUN}`;
const NS_GLOBAL = `${NS}g`;
const EVERY = "every-minute";
const SECONDS = "every-second";
const READER_EMAIL = `e2e-w7f-reader-${RUN}@egret.local`;
const READER_PASSWORD = "ReaderPw12345!";
const READER_ROLE = `e2e_w7f_reader_${RUN}`;

/** One SQL statement on the private database; rows as `|`-separated lines (`psql -At`). */
function sql(q: string): string {
  return execFileSync("docker", ["exec", PG_CONTAINER, "psql", "-U", "egret", "-d", PG_DB, "-Atc", q], { encoding: "utf8" }).trim();
}
const sqlOne = (q: string): string => sql(q).split("\n")[0] ?? "";
const lit = (s: string) => `'${s.replace(/'/g, "''")}'`;
const dbNow = () => Number(sqlOne("SELECT extract(epoch FROM now())")) * 1000;

/** A schedule; `inputs` is its block text (already indented) for the values it supplies. */
type Sched = { id: string; cron: string; withSeconds?: boolean; inputs?: string };
/** A flow's YAML: its schedules, and optionally its inputs and labels (block text, already indented). */
function flowYaml(id: string, ns: string, scheds: Sched[], extra: { inputs?: string; labels?: string } = {}): string {
  const triggers = scheds
    .map((s) => `  - id: ${s.id}\n    type: io.kestra.plugin.core.trigger.Schedule\n    cron: "${s.cron}"${s.withSeconds ? "\n    withSeconds: true" : ""}${s.inputs ? `\n${s.inputs}` : ""}`)
    .join("\n");
  return `id: ${id}\nnamespace: ${ns}\n${extra.labels ?? ""}${extra.inputs ?? ""}tasks:\n  - id: hello\n    type: io.kestra.plugin.core.log.Log\n    message: backfill\ntriggers:\n${triggers}\n`;
}

/** The backfill of one trigger, read from the database; null when it has none. */
type Bf = { id: string; total: number; done: number; paused: boolean; pausedAt: number | null; start: number; end: number; prev: number; labels: string };
function backfillOf(flowUuid: string, triggerId: string): Bf | null {
  const line = sqlOne(
    `SELECT id, total_ticks, ticks_done, paused, coalesce(extract(epoch FROM paused_at)::text, ''),
            extract(epoch FROM window_start)::text, extract(epoch FROM window_end)::text, labels::text,
            extract(epoch FROM previous_next_evaluation_date)::text
       FROM workflow.trigger_backfills WHERE flow_id = ${lit(flowUuid)} AND trigger_id = ${lit(triggerId)}`,
  );
  if (!line) return null;
  const [id, total, done, paused, pausedAt, start, end, labels, prev] = line.split("|");
  return {
    id,
    total: Number(total),
    done: Number(done),
    paused: paused === "t",
    pausedAt: pausedAt ? Number(pausedAt) * 1000 : null,
    start: Number(start) * 1000,
    end: Number(end) * 1000,
    prev: Number(prev) * 1000,
    labels,
  };
}

/** The schedule's stored position on the queue (ms), read from the database. */
function nextOf(flowUuid: string, triggerId: string): number | null {
  const v = sqlOne(`SELECT coalesce(extract(epoch FROM next_evaluation_date)::text, '') FROM workflow.triggers WHERE flow_id = ${lit(flowUuid)} AND trigger_id = ${lit(triggerId)}`);
  return v ? Number(v) * 1000 : null;
}

/** Executions one backfill fired, as their scheduled dates (ms, ascending) and their inputs' `who`. */
function backfillExecutions(backfillId: string): { dates: number[]; who: string[] } {
  const out = sql(
    `SELECT extract(epoch FROM scheduled_date)::text || '|' || coalesce(inputs->>'who', '') FROM workflow.executions
      WHERE backfill_id = ${lit(backfillId)} ORDER BY scheduled_date`,
  );
  const rows = out ? out.split("\n").map((l) => l.split("|")) : [];
  return { dates: rows.map((r) => Math.round(Number(r[0]) * 1000)), who: rows.map((r) => r[1]) };
}
const backfillExecCount = (backfillId: string) => Number(sqlOne(`SELECT count(*) FROM workflow.executions WHERE backfill_id = ${lit(backfillId)}`));

/** The regular (no backfill) executions of one trigger, as scheduled dates (ms, ascending). */
function regularDates(flowUuid: string, triggerId: string): number[] {
  const out = sql(
    `SELECT extract(epoch FROM scheduled_date)::text FROM workflow.executions
      WHERE flow_id = ${lit(flowUuid)} AND execution_trigger->>'id' = ${lit(triggerId)} AND backfill_id IS NULL AND scheduled_date IS NOT NULL
      ORDER BY scheduled_date`,
  );
  return out ? out.split("\n").map((s) => Math.round(Number(s) * 1000)) : [];
}

/**
 * Wait, bounded by the database's clock, until `now()` is past `at`: the bound is the distance to
 * `at` plus a margin, computed from the clock, not a sleep tuned to this machine.
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

const minuteFloor = (ms: number) => Math.floor(ms / 60_000) * 60_000;
const secondFloor = (ms: number) => Math.floor(ms / 1000) * 1000;
/**
 * The browser's local `datetime-local` text for an instant (the form converts local to UTC). The
 * seconds are left out when they are zero: that is the browser's canonical form, and Playwright's
 * `fill` rejects `HH:mm:00` as "Malformed value".
 */
function localInput(ms: number): string {
  const d = new Date(ms);
  const p = (n: number) => String(n).padStart(2, "0");
  const base = `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())}T${p(d.getHours())}:${p(d.getMinutes())}`;
  return d.getSeconds() === 0 ? base : `${base}:${p(d.getSeconds())}`;
}

/** An element's bounding box in CSS pixels (the viewport's origin), or null when it is not rendered. */
async function boxOf(l: Locator): Promise<{ x: number; y: number; w: number; h: number } | null> {
  const b = await l.first().boundingBox();
  return b && { x: Math.round(b.x), y: Math.round(b.y), w: Math.round(b.width), h: Math.round(b.height) };
}

async function shot(page: Page, key: string): Promise<void> {
  if (SHOTS) await page.screenshot({ path: `${SHOTS}/w7f-${key.replace(/\W+/g, "-")}.png`, fullPage: true });
}

/** Light or dark, through the app's own preference (`ThemeProvider`, `egret-theme`), then a reload. */
async function setTheme(page: Page, theme: "light" | "dark"): Promise<void> {
  await page.evaluate((t) => localStorage.setItem("egret-theme", t), theme);
  await page.reload();
  await expect(page.locator("html")).toHaveClass(theme === "dark" ? /\bdark\b/ : /^(?!.*\bdark\b)/);
}

/** `primary`: painted on the shared `--primary` surface (a default button), held to 3:1 here. */
type TextSample = { text: string; fg: string; bg: string; ratio: number; primary: boolean; disabled: boolean };
type Measure = {
  viewport: number;
  pageScrollWidth: number;
  scopeLeft: number;
  scopeRight: number;
  clipped: string[];
  scrollers: { element: string; width: number; scrollWidth: number }[];
  samples: TextSample[];
};

/**
 * Measure `scope` in the DOM: page overflow, clipped text, horizontal scrollers, and the contrast
 * of each text leaf against the first opaque surface behind it (colours resolved through a canvas,
 * so `oklch()` tokens compare as the sRGB the screen shows).
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
    const samples: TextSample[] = [];
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
      // A disabled control's text is exempt from the contrast minimum (WCAG 1.4.3): reported, not held.
      const disabled = el.closest("button:disabled, [aria-disabled='true'], input:disabled") !== null;
      samples.push({ text, fg: cs.color, bg: `rgb(${bg.map(Math.round).join(", ")})`, ratio: Math.round(((l1 + 0.05) / (l2 + 0.05)) * 100) / 100, primary: onPrimary, disabled });
    }
    const scrollers = Array.from(root.querySelectorAll<HTMLElement>("*"))
      .filter((e) => {
        const ox = getComputedStyle(e).overflowX;
        return (ox === "auto" || ox === "scroll") && e.scrollWidth > e.clientWidth;
      })
      .map((e) => ({ element: [e.tagName.toLowerCase(), ...Array.from(e.classList).slice(0, 4)].join("."), width: e.clientWidth, scrollWidth: e.scrollWidth }));
    const sr = root.getBoundingClientRect();
    return {
      viewport: vw,
      pageScrollWidth: document.documentElement.scrollWidth,
      scopeLeft: Math.round(sr.left),
      scopeRight: Math.round(sr.right),
      clipped,
      scrollers,
      samples,
    };
  });
}

/**
 * The assertions every measured view must meet: no horizontal page scroll, the scope inside the
 * viewport, no text clipped, and every sampled text at 4.5:1 or more against its surface (3:1 on
 * the shared `--primary`, which is the design system's, as the W7e spec measures it).
 */
async function expectReadable(page: Page, scope: Locator, what: string): Promise<Measure> {
  await page.waitForTimeout(400); // a toast or a fading row would measure a moment, not the view
  const m = await measure(scope);
  const worst = [...m.samples].sort((a, b) => a.ratio - b.ratio).slice(0, 3);
  report(what, { viewport: m.viewport, pageScrollWidth: m.pageScrollWidth, scope: [m.scopeLeft, m.scopeRight], clipped: m.clipped, scrollers: m.scrollers, texts: m.samples.length, worst });
  await shot(page, what);
  expect(m.pageScrollWidth, `${what}: the page scrolls sideways`).toBeLessThanOrEqual(m.viewport);
  expect(m.scopeLeft, `${what}: starts inside the viewport`).toBeGreaterThanOrEqual(0);
  expect(m.scopeRight, `${what}: ends inside the viewport`).toBeLessThanOrEqual(m.viewport + 1);
  expect(m.clipped, `${what}: clipped text`).toEqual([]);
  expect(m.samples.length, `${what}: texts measured`).toBeGreaterThan(0);
  for (const s of m.samples.filter((x) => !x.disabled)) expect(s.ratio, `${what}: "${s.text}" ${s.fg} on ${s.bg}`).toBeGreaterThanOrEqual(s.primary ? 3 : 4.5);
  return m;
}

const row = (page: Page, triggerId: string): Locator => page.getByTestId("triggers-table").locator(`tr[data-trigger="${triggerId}"]`);
const strip = (page: Page): Locator => page.getByTestId("backfill-strip");
const switchOf = (page: Page, name: string): Locator => page.getByRole("switch", { name: `Trigger ${name} enabled`, exact: true });
const drawer = (page: Page): Locator => page.getByRole("dialog").filter({ has: page.getByRole("heading", { name: /^Trigger / }) });
const dialogFor = (page: Page, triggerId: string): Locator => page.getByRole("dialog", { name: `Backfill ${triggerId}`, exact: true });
/** A row on the global page: its checkbox names the trigger and its flow. */
const globalRowNs = (page: Page, triggerId: string, flowId: string, ns: string): Locator =>
  page.getByTestId("triggers-table").locator("tbody tr").filter({ has: page.getByRole("checkbox", { name: `Select ${triggerId} of ${ns}.${flowId}` }) });

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

/** Fill the create form's Start and End (local text), and submit it. */
async function startBackfillUi(dialog: Locator, triggerId: string, start: string, end: string | null): Promise<void> {
  await dialog.getByLabel("Start", { exact: true }).fill(start);
  if (end !== null) await dialog.getByLabel("End", { exact: true }).fill(end);
  await dialog.getByRole("button", { name: "Start backfill", exact: true }).click();
  await expect(dialog, `${triggerId}: the form closes once the backfill is created`).toBeHidden({ timeout: 20_000 });
}

test.describe("W7f backfill: §6 scenarios 1-7 and the measurement", () => {
  test.skip(!ENABLED, "opt-in: set EGRET_E2E_WORKFLOWS=1 with the private stack up (see the header)");
  test.setTimeout(900_000);

  let ws = "";
  let api = "";
  let base = "";

  /** Sign in as the admin, and read the workspace the API and the pages use. */
  async function adminSession(page: Page, context: import("@playwright/test").BrowserContext) {
    await page.setViewportSize({ width: 1280, height: 900 });
    await loginLocalAdmin(page);
    await setTheme(page, "light");
    ws = (await firstWorkspace(page.request)).slug;
    api = `/api/ws/${ws}/workflows`;
    base = `/explore/${ws}/workflows`;
    return csrf(context);
  }

  test("1. a five-minute backfill with an input and labels: five executions, the overview, light and dark", async ({ page, context }) => {
    page.on("pageerror", (e) => console.log(`[pageerror] ${e.message}`));
    const h = await adminSession(page, context);
    const flow = await postJson(
      page.request,
      `${api}/flows`,
      h,
      { source: flowYaml(`s1-${RUN}`, NS, [{ id: EVERY, cron: "* * * * *" }], { labels: "labels:\n  team: data\n", inputs: "inputs:\n  - id: who\n    type: STRING\n    defaults: hello\n" }) },
    );
    const flowId: string = flow.id;
    await page.goto(`${base}/flows/${flowId}/triggers`);
    const r = row(page, EVERY);
    await expect(r).toBeVisible();
    await r.getByRole("button", { name: "Backfill", exact: true }).click();
    const dialog = dialogFor(page, EVERY);
    await expect(dialog).toBeVisible();

    // The last five minutes: five whole-minute ticks, the last one a second before the current minute began.
    const t0 = minuteFloor(Date.now());
    const start = t0 - 5 * 60_000;
    const end = t0 - 1_000;
    await dialog.locator(`[id="${EVERY}-backfill-who"]`).fill("ops run");
    await dialog.getByRole("button", { name: "Add label", exact: true }).click();
    await dialog.locator(`[id="${EVERY}-backfill-label-key-0"]`).fill("team");
    await dialog.locator(`[id="${EVERY}-backfill-label-value-0"]`).fill("ops");
    await dialog.getByRole("button", { name: "Add label", exact: true }).click();
    await dialog.locator(`[id="${EVERY}-backfill-label-key-1"]`).fill("reason");
    await dialog.locator(`[id="${EVERY}-backfill-label-value-1"]`).fill("test");
    await startBackfillUi(dialog, EVERY, localInput(start), localInput(end));

    const bf = backfillOf(flowId, EVERY);
    expect(bf, "the backfill is on the database").not.toBeNull();
    expect(bf!.total, "five ticks in the window").toBe(5);
    expect(JSON.parse(bf!.labels), "the labels as sent").toEqual([
      { key: "team", value: "ops" },
      { key: "reason", value: "test" },
    ]);
    report("scenario 1: backfill", { id: bf!.id, total: bf!.total, start: new Date(bf!.start).toISOString(), end: new Date(bf!.end).toISOString() });

    // The strip is on the tab while the five ticks fire.
    await expect(strip(page), "the progress strip appears under the trigger").toBeVisible({ timeout: 10_000 });
    await expect(strip(page)).toContainText("of 5 ticks");

    // Five executions appear, one per tick, each with the backfill's input.
    await expect.poll(() => backfillExecCount(bf!.id), { timeout: 180_000, intervals: [1_000] }).toBe(5);
    const fired = backfillExecutions(bf!.id);
    expect(fired.dates, "one execution per tick, in order").toEqual([0, 1, 2, 3, 4].map((i) => t0 - (5 - i) * 60_000));
    expect(new Set(fired.who), "every execution carries the backfill's input").toEqual(new Set(["ops run"]));

    // The strip goes away once the window is done (the row is deleted, the tab refetches).
    await expect.poll(() => backfillOf(flowId, EVERY), { timeout: 60_000 }).toBeNull();
    await page.reload();
    await expect(strip(page), "the strip is gone").toHaveCount(0);
    await expect(r.getByRole("button", { name: "Backfill", exact: true })).toBeEnabled();

    // Each execution's overview shows the merged labels and the Backfill mark, in both themes.
    const execId = sqlOne(`SELECT id FROM workflow.executions WHERE backfill_id = ${lit(bf!.id)} ORDER BY scheduled_date LIMIT 1`);
    for (const theme of ["light", "dark"] as const) {
      await setTheme(page, theme);
      await page.goto(`${base}/executions/${execId}/overview`);
      const labels = page.getByRole("list", { name: "Labels" });
      await expect(labels.getByText("team: ops", { exact: true }), `${theme}: the backfill's label wins over the flow's`).toBeVisible();
      await expect(labels.getByText("reason: test", { exact: true }), `${theme}: the backfill's second label`).toBeVisible();
      await expect(labels.getByText("team: data", { exact: true }), `${theme}: the flow's value is overridden`).toHaveCount(0);
      await expect(page.getByText("Backfill", { exact: true }).first(), `${theme}: the Backfill mark`).toBeVisible();
    }
    await setTheme(page, "light");
  });

  test("2. re-run: a window the schedule already ran fires a second execution per tick", async ({ page, context }) => {
    const h = await adminSession(page, context);
    const flow = await postJson(page.request, `${api}/flows`, h, { source: flowYaml(`s2-${RUN}`, NS, [{ id: EVERY, cron: "* * * * *" }]) });
    const flowId: string = flow.id;
    // Two regular ticks must have fired: the schedule's own clock, bounded by two minutes plus a margin.
    await expect.poll(() => regularDates(flowId, EVERY).length, { timeout: 200_000, intervals: [2_000] }).toBeGreaterThanOrEqual(2);
    const [d1, d2] = regularDates(flowId, EVERY);
    report("scenario 2: regular ticks already fired", { d1: new Date(d1).toISOString(), d2: new Date(d2).toISOString() });

    await page.goto(`${base}/flows/${flowId}/triggers`);
    await row(page, EVERY).getByRole("button", { name: "Backfill", exact: true }).click();
    await startBackfillUi(dialogFor(page, EVERY), EVERY, localInput(d1), localInput(d2));
    const bf = backfillOf(flowId, EVERY);
    expect(bf!.total, "the window is exactly the two ticks that ran").toBe(2);

    await expect.poll(() => backfillExecCount(bf!.id), { timeout: 120_000, intervals: [1_000] }).toBe(2);
    await expect.poll(() => backfillOf(flowId, EVERY), { timeout: 60_000 }).toBeNull();
    // For each of the two ticks: the regular execution and the backfill's, two in all.
    for (const d of [d1, d2]) {
      const counts = sqlOne(
        `SELECT count(*) || '|' || count(backfill_id) FROM workflow.executions
          WHERE flow_id = ${lit(flowId)} AND execution_trigger->>'id' = ${lit(EVERY)} AND scheduled_date = to_timestamp(${d / 1000})`,
      ).split("|");
      report("scenario 2: executions for one tick", { tick: new Date(d).toISOString(), all: counts[0], fromBackfill: counts[1] });
      expect(counts, `tick ${new Date(d).toISOString()}: one regular and one from the backfill`).toEqual(["2", "1"]);
    }
  });

  test("3. seconds cron, one-hour window: pause stops it, resume continues, delete leaves the regular schedule", async ({ page, context }) => {
    const h = await adminSession(page, context);
    const flow = await postJson(page.request, `${api}/flows`, h, {
      source: flowYaml(`s3-${RUN}`, NS, [{ id: SECONDS, cron: "* * * * * *", withSeconds: true }]),
    });
    const flowId: string = flow.id;
    await page.goto(`${base}/flows/${flowId}/triggers`);
    const r = row(page, SECONDS);
    await r.getByRole("button", { name: "Backfill", exact: true }).click();
    const dialog = dialogFor(page, SECONDS);
    // One hour back, to now (the end left empty). Whole seconds: the form takes `step=1`.
    const start = secondFloor(Date.now() - 3_600_000);
    await startBackfillUi(dialog, SECONDS, localInput(start), null);
    const bf = backfillOf(flowId, SECONDS);
    expect(bf!.total, "an hour of seconds, about 3600 ticks").toBeGreaterThan(3500);
    await expect(strip(page)).toBeVisible({ timeout: 10_000 });

    // Pause: the strip shows it, the database says so.
    await strip(page).getByRole("button", { name: "Pause", exact: true }).click();
    await expect.poll(() => backfillOf(flowId, SECONDS)?.paused ?? false, { timeout: 30_000 }).toBe(true);
    await expect(strip(page)).toContainText("Paused");
    const paused = backfillOf(flowId, SECONDS)!;
    await waitForDbClock(paused.pausedAt! + 5_000, "after the pause");
    const c1 = backfillExecCount(paused.id);
    const t1 = backfillOf(flowId, SECONDS)!.done;
    await waitForDbClock(dbNow() + 5_000, "a further five seconds while paused");
    const c2 = backfillExecCount(paused.id);
    const t2 = backfillOf(flowId, SECONDS)!.done;
    report("scenario 3: while paused", { executionsBefore: c1, executionsAfter: c2, ticksBefore: t1, ticksAfter: t2 });
    expect(c2, "no new executions while paused").toBe(c1);
    expect(t2, "no tick advanced while paused").toBe(t1);

    // Resume: the executions continue.
    await strip(page).getByRole("button", { name: "Resume", exact: true }).click();
    await expect.poll(() => backfillOf(flowId, SECONDS)?.paused ?? true, { timeout: 30_000 }).toBe(false);
    await expect.poll(() => backfillOf(flowId, SECONDS)?.done ?? 0, { timeout: 60_000, intervals: [1_000] }).toBeGreaterThan(t2);
    await expect(strip(page)).not.toContainText("Paused");

    // Delete, with the confirm; the strip goes, and the trigger is on its regular schedule.
    await strip(page).getByRole("button", { name: "Delete", exact: true }).click();
    const confirm = page.getByRole("alertdialog", { name: "Delete this backfill?" });
    await expect(confirm).toBeVisible();
    await expect(confirm).toContainText(/Its remaining [\d,]+ ticks will not run\. Executions already started stay\./);
    const beforeDelete = backfillOf(flowId, SECONDS)!;
    await confirm.getByRole("button", { name: "Delete backfill", exact: true }).click();
    await expect.poll(() => backfillOf(flowId, SECONDS), { timeout: 30_000 }).toBeNull();
    await expect(strip(page)).toHaveCount(0);
    const next = nextOf(flowId, SECONDS);
    report("scenario 3: after delete", {
      windowEnd: new Date(beforeDelete.end).toISOString(),
      regularPosition: new Date(beforeDelete.prev).toISOString(),
      next: next === null ? null : new Date(next).toISOString(),
      regularFired: regularDates(flowId, SECONDS).length,
    });
    expect(next, "the regular schedule is back on the queue").not.toBeNull();
    // The regular schedule resumes from its own position (the tick it had when the backfill was
    // created, `previous_next_evaluation_date`), never from a tick inside the window (spec §3.6).
    expect(next!, "the regular next run is not before the regular position").toBeGreaterThanOrEqual(beforeDelete.prev);
    await expect(r.getByRole("button", { name: "Backfill", exact: true })).toBeEnabled();
  });

  test("4. the form: future dates refused, Start after End, a missing input, a second create answers 409", async ({ page, context }) => {
    const h = await adminSession(page, context);
    const flow = await postJson(page.request, `${api}/flows`, h, {
      // The schedule supplies `who` (a required input with no default): otherwise its own regular
      // ticks fail, take the trigger off the queue, and the create is refused for that instead.
      source: flowYaml(`s4-${RUN}`, NS, [{ id: EVERY, cron: "* * * * *", inputs: "    inputs:\n      who: sched" }], { inputs: "inputs:\n  - id: who\n    type: STRING\n" }),
    });
    const flowId: string = flow.id;
    await page.goto(`${base}/flows/${flowId}/triggers`);
    await row(page, EVERY).getByRole("button", { name: "Backfill", exact: true }).click();
    const dialog = dialogFor(page, EVERY);
    const submit = dialog.getByRole("button", { name: "Start backfill", exact: true });
    const now = Date.now();

    // The pickers stop at now: the native max is the current local time.
    const max = await dialog.getByLabel("Start", { exact: true }).getAttribute("max");
    expect(max, "the start's max").not.toBeNull();
    expect(Math.abs(new Date(max!).getTime() - now), "max is now, to the minute").toBeLessThan(60_000);
    expect(await dialog.getByLabel("End", { exact: true }).getAttribute("max")).toBe(max);

    // A start is needed first.
    await expect(submit).toBeDisabled();
    await expect(submit).toHaveAccessibleDescription("Pick a start");

    // An end in the future: refused, whatever the picker shows (the form checks it too).
    await dialog.getByLabel("Start", { exact: true }).fill(localInput(now - 3_600_000));
    await dialog.getByLabel("End", { exact: true }).fill(localInput(now + 3_600_000));
    await expect(submit).toBeDisabled();
    await expect(submit).toHaveAccessibleDescription("The end is in the future; leave it empty to end now");

    // Start after End: disabled with its reason.
    await dialog.getByLabel("End", { exact: true }).fill(localInput(now - 600_000));
    await dialog.getByLabel("Start", { exact: true }).fill(localInput(now - 300_000));
    await expect(submit).toBeDisabled();
    await expect(submit).toHaveAccessibleDescription("The start must be before the end");

    // A required input with no value (the form pre-fills it from the schedule; clear it): disabled, and
    // the field says so.
    await dialog.getByLabel("Start", { exact: true }).fill(localInput(now - 600_000));
    await dialog.getByLabel("End", { exact: true }).fill("");
    await expect(dialog.locator(`[id="${EVERY}-backfill-who"]`)).toHaveValue("sched");
    await dialog.locator(`[id="${EVERY}-backfill-who"]`).fill("");
    await expect(submit).toBeDisabled();
    await expect(submit).toHaveAccessibleDescription("Fix the inputs");
    await expect(dialog.locator(`[id="${EVERY}-backfill-who"]`)).toHaveAttribute("aria-invalid", "true");

    // Filled: enabled. The first create goes through the form, over two hours of minute ticks: long
    // enough to still be running when the second create below is sent (a ten-minute window can finish
    // before the check, which would not test a conflict at all).
    await dialog.getByLabel("Start", { exact: true }).fill(localInput(now - 2 * 3_600_000));
    await dialog.locator(`[id="${EVERY}-backfill-who"]`).fill("x");
    await expect(submit).toBeEnabled();
    await submit.click();
    await expect(dialog).toBeHidden({ timeout: 20_000 });
    const bf = backfillOf(flowId, EVERY);
    expect(bf, "the first backfill exists").not.toBeNull();
    expect(bf!.total, "two hours of minute ticks").toBeGreaterThan(100);

    // A second create for the same trigger answers 409, through the backend's own port.
    const again = await page.request.post(`${api}/flows/${flowId}/triggers/${EVERY}/backfill`, {
      headers: h,
      data: { start: new Date(now - 600_000).toISOString(), inputs: { who: "x" } },
    });
    const body = await again.text();
    report("scenario 4: second create", { status: again.status(), body });
    expect(again.status()).toBe(409);
    expect(body).toContain("backfill_exists");
  });

  test("5. the global page: filter by backfill, bulk pause two, reload, bulk delete", async ({ page, context }) => {
    const h = await adminSession(page, context);
    const ids = [`g1-${RUN}`, `g2-${RUN}`];
    const flowIds: string[] = [];
    for (const id of ids) {
      const f = await postJson(page.request, `${api}/flows`, h, {
        source: flowYaml(id, NS_GLOBAL, [{ id: SECONDS, cron: "* * * * * *", withSeconds: true }]),
      });
      flowIds.push(f.id);
    }
    // Two-hour windows of seconds: still running for the whole test.
    for (const fid of flowIds) {
      await postJson(page.request, `${api}/flows/${fid}/triggers/${SECONDS}/backfill`, h, {
        start: new Date(secondFloor(Date.now() - 2 * 3_600_000)).toISOString(),
      });
    }

    await page.goto(`${base}/triggers?namespace=${encodeURIComponent(NS_GLOBAL)}&backfill=running`);
    const list = page.getByTestId("triggers-table");
    const triggerRows = list.locator('tbody tr:not([data-testid="backfill-strip"])');
    await expect(triggerRows).toHaveCount(2);
    for (const id of ids) {
      const fid = flowIds[ids.indexOf(id)];
      await expect(globalRowNs(page, SECONDS, id, NS_GLOBAL)).toContainText("Running");
      report("scenario 5: running", { flow: id, flowUuid: fid });
    }

    // Bulk pause the two.
    await page.getByRole("checkbox", { name: `Select ${SECONDS} of ${NS_GLOBAL}.${ids[0]}` }).click();
    await page.getByRole("checkbox", { name: `Select ${SECONDS} of ${NS_GLOBAL}.${ids[1]}` }).click();
    await expect(page.getByText("2 triggers selected")).toBeVisible();
    const bulk = page.waitForResponse((r) => r.url().endsWith(`${api}/triggers/backfill/paused`) && r.request().method() === "PUT");
    await page.getByRole("button", { name: "Pause backfills", exact: true }).click();
    const answer = await (await bulk).json();
    report("scenario 5: bulk pause answer", answer.results?.map((x: { trigger_id: string; ok: boolean }) => `${x.trigger_id}:${x.ok}`));
    expect((answer.results ?? []).every((x: { ok: boolean }) => x.ok), "both pauses answered ok").toBe(true);
    // Paused backfills leave the "running" view: it is empty for these two now.
    await expect(triggerRows).toHaveCount(0);
    for (const fid of flowIds) expect(backfillOf(fid, SECONDS)?.paused, "paused in the database").toBe(true);

    // Reload on the "paused" facet: both rows say Paused.
    await page.goto(`${base}/triggers?namespace=${encodeURIComponent(NS_GLOBAL)}&backfill=paused`);
    await expect(triggerRows).toHaveCount(2);
    for (const id of ids) await expect(globalRowNs(page, SECONDS, id, NS_GLOBAL)).toContainText("Paused");

    // Bulk delete the two, from the same view. The database has no backfill left for them.
    await page.getByRole("checkbox", { name: `Select ${SECONDS} of ${NS_GLOBAL}.${ids[0]}` }).click();
    await page.getByRole("checkbox", { name: `Select ${SECONDS} of ${NS_GLOBAL}.${ids[1]}` }).click();
    await page.getByRole("button", { name: "Delete backfills", exact: true }).click();
    // Bulk delete confirms first (FE-ACTION-1): the confirm's own action deletes.
    await page.getByRole("alertdialog").getByRole("button", { name: "Delete backfills", exact: true }).click();
    await expect.poll(() => flowIds.map((fid) => backfillOf(fid, SECONDS)).filter((b) => b !== null).length, { timeout: 30_000 }).toBe(0);
    await page.goto(`${base}/triggers?namespace=${encodeURIComponent(NS_GLOBAL)}&backfill=paused`);
    await expect(triggerRows).toHaveCount(0);
  });

  test("6. a viewer without workflow:manage: the Backfill button and the strip's actions disabled with the reason", async ({ page, context, browser }) => {
    const h = await adminSession(page, context);
    const flow = await postJson(page.request, `${api}/flows`, h, {
      source: flowYaml(`s6-${RUN}`, NS, [
        { id: "six-run", cron: "* * * * *" },
        { id: "six-idle", cron: "0 4 * * *" },
      ]),
    });
    const flowId: string = flow.id;
    // A paused backfill, so the strip is still there when the viewer looks.
    await postJson(page.request, `${api}/flows/${flowId}/triggers/six-run/backfill`, h, {
      start: new Date(secondFloor(Date.now() - 3_600_000)).toISOString(),
    });
    const bf = backfillOf(flowId, "six-run")!;
    await page.request.put(`${api}/flows/${flowId}/triggers/six-run/backfill/paused`, { headers: h, data: { paused: true } });
    expect(backfillOf(flowId, "six-run")?.paused, "paused before the viewer looks").toBe(true);

    const viewerRole = await postJson(page.request, "/api/control-plane/access/roles", h, {
      scope: "workspace",
      name: READER_ROLE,
      description: "W7f e2e: workflow:read without workflow:manage",
      parentRoleId: sqlOne("SELECT id FROM roles WHERE name = 'ws_viewer'"),
    });
    const perms = await page.request.put(`/api/control-plane/access/roles/${viewerRole.id}/permissions`, { headers: h, data: { permissions: ["workflow:read"] } });
    expect(perms.ok(), `role permissions → ${perms.status()}`).toBeTruthy();
    await postJson(page.request, "/api/control-plane/users", h, { email: READER_EMAIL, name: "Backfill Reader", password: READER_PASSWORD });
    await postJson(page.request, `/api/ws/${ws}/members`, h, { principal_type: "user", principal_id: READER_EMAIL, role: READER_ROLE });

    const ctx = await browser.newContext({ baseURL: test.info().project.use.baseURL as string, viewport: { width: 1280, height: 900 } });
    const viewer = await ctx.newPage();
    await loginAs(viewer, READER_EMAIL, READER_PASSWORD);
    await viewer.goto(`${base}/flows/${flowId}/triggers`);
    await expect(row(viewer, "six-run")).toBeVisible();

    // The strip's Resume and Delete: disabled, with the reason.
    const s = strip(viewer);
    await expect(s.getByRole("button", { name: "Resume", exact: true })).toBeDisabled();
    await expect(s.getByRole("button", { name: "Resume", exact: true })).toHaveAccessibleDescription("You need workflow:manage");
    await expect(s.getByRole("button", { name: "Delete", exact: true })).toBeDisabled();
    await expect(s.getByRole("button", { name: "Delete", exact: true })).toHaveAccessibleDescription("You need workflow:manage");
    // The idle schedule's Backfill button: disabled, with the reason.
    const idle = row(viewer, "six-idle").getByRole("button", { name: "Backfill", exact: true });
    await expect(idle).toBeDisabled();
    await expect(idle).toHaveAccessibleDescription("You need workflow:manage");
    // Still paused, the same backfill, after the viewer's look.
    expect(backfillOf(flowId, "six-run")?.id).toBe(bf.id);
    expect(backfillOf(flowId, "six-run")?.paused).toBe(true);
    await ctx.close();
  });

  test("7. the trigger switched off during a backfill: frozen; on again: it resumes", async ({ page, context }) => {
    const h = await adminSession(page, context);
    const flow = await postJson(page.request, `${api}/flows`, h, {
      source: flowYaml(`s7-${RUN}`, NS, [{ id: SECONDS, cron: "* * * * * *", withSeconds: true }]),
    });
    const flowId: string = flow.id;
    await postJson(page.request, `${api}/flows/${flowId}/triggers/${SECONDS}/backfill`, h, {
      start: new Date(secondFloor(Date.now() - 2 * 3_600_000)).toISOString(),
    });
    await page.goto(`${base}/flows/${flowId}/triggers`);
    await expect(strip(page)).toBeVisible();

    await switchOf(page, SECONDS).click();
    await expect(switchOf(page, SECONDS)).toHaveAttribute("aria-checked", "false");
    await expect(strip(page)).toContainText("Frozen while the trigger is off");
    // Off: no tick advances (the scheduler skips a switched-off trigger). Measured by the database clock.
    const off = backfillOf(flowId, SECONDS)!;
    await waitForDbClock(dbNow() + 5_000, "frozen for five seconds");
    const frozenFor = backfillOf(flowId, SECONDS)!;
    report("scenario 7: frozen", { before: off.done, after: frozenFor.done });
    expect(frozenFor.done, "no tick while the trigger is off").toBe(off.done);

    // On: the enable dialog asks about the missed runs; "Skip" (the default) leaves the backfill's
    // dates alone. The strip stops saying frozen, and the ticks move again.
    await switchOf(page, SECONDS).click();
    const enable = page.getByRole("dialog", { name: "Enable trigger" });
    await expect(enable).toBeVisible();
    await expect(enable.getByRole("radio", { name: "Skip missed runs; start at the next tick" })).toBeChecked();
    await enable.getByRole("button", { name: "Enable", exact: true }).click();
    await expect(enable).toBeHidden();
    await expect(switchOf(page, SECONDS)).toHaveAttribute("aria-checked", "true");
    await expect(strip(page)).not.toContainText("Frozen while the trigger is off");
    await expect.poll(() => backfillOf(flowId, SECONDS)?.done ?? 0, { timeout: 60_000, intervals: [1_000] }).toBeGreaterThan(frozenFor.done);
  });

  test("measure: the tab's column and strip, the drawer's Backfill section, the create dialog, the global page", async ({ page, context }) => {
    test.setTimeout(600_000);
    const h = await adminSession(page, context);
    const flow = await postJson(page.request, `${api}/flows`, h, {
      source: flowYaml(`m-${RUN}`, NS, [
        { id: "m-run", cron: "* * * * *" },
        { id: "m-idle", cron: "0 5 * * *" },
      ]),
    });
    const flowId: string = flow.id;
    await postJson(page.request, `${api}/flows/${flowId}/triggers/m-run/backfill`, h, {
      start: new Date(minuteFloor(Date.now()) - 2 * 3_600_000).toISOString(),
      labels: [{ key: "team", value: "ops" }],
    });
    await page.request.put(`${api}/flows/${flowId}/triggers/m-run/backfill/paused`, { headers: h, data: { paused: true } });
    const base0 = base;
    for (const theme of ["light", "dark"] as const) {
      await setTheme(page, theme);
      for (const width of [1280, 390]) {
        await page.setViewportSize({ width, height: 900 });
        await page.goto(`${base0}/flows/${flowId}/triggers`);
        await expect(strip(page)).toBeVisible();
        await expectReadable(page, page.getByRole("tabpanel"), `tab ${theme} ${width}`);
        report(`box tab ${theme} ${width}`, {
          column: await boxOf(page.getByRole("columnheader", { name: "Backfill", exact: true })),
          strip: await boxOf(strip(page)),
          idleButton: await boxOf(row(page, "m-idle").getByRole("button", { name: "Backfill", exact: true })),
          table: await boxOf(page.getByTestId("triggers-table")),
        });
        await page.goto(`${base0}/flows/${flowId}/triggers?trigger=m-run`);
        await expect(drawer(page).getByRole("heading", { name: "Backfill", exact: true })).toBeVisible();
        await expectReadable(page, drawer(page), `drawer m-run ${theme} ${width}`);
        report(`box drawer ${theme} ${width}`, {
          drawer: await boxOf(drawer(page)),
          backfillHeading: await boxOf(drawer(page).getByRole("heading", { name: "Backfill", exact: true })),
        });
        await page.goto(`${base0}/flows/${flowId}/triggers`);
        await row(page, "m-idle").getByRole("button", { name: "Backfill", exact: true }).click();
        await expect(dialogFor(page, "m-idle")).toBeVisible();
        await expectReadable(page, dialogFor(page, "m-idle"), `dialog ${theme} ${width}`);
        report(`box dialog ${theme} ${width}`, { dialog: await boxOf(dialogFor(page, "m-idle")) });
        await page.keyboard.press("Escape");
        await expect(dialogFor(page, "m-idle")).toBeHidden();
        await page.goto(`${base0}/triggers?namespace=${encodeURIComponent(NS)}`);
        await expect(page.getByTestId("triggers-table").locator("tbody tr").first()).toBeVisible();
        await expectReadable(page, page.locator("main"), `global ${theme} ${width}`);
        report(`box global ${theme} ${width}`, {
          table: await boxOf(page.getByTestId("triggers-table")),
          backfillColumn: await boxOf(page.getByRole("columnheader", { name: "Backfill", exact: true })),
          scroller: await boxOf(page.getByTestId("triggers-table").locator("xpath=ancestor::div[contains(@class,'overflow-x-auto')][1]")),
        });
      }
    }
    await page.setViewportSize({ width: 1280, height: 900 });
    await setTheme(page, "light");
  });
});
