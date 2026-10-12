import { test, expect, type Locator, type Page } from "@playwright/test";
import { csrf, firstWorkspace, loginLocalAdmin, postJson } from "./helpers";

/**
 * W7b (workflow UI, executions) end to end: the list and its child switch,
 * the gantt's attempt segments, logs that follow until the execution is
 * complete, a run's own logs, a file preview, Resume before a pause's
 * deadline, and Replay (spec `docs/en/specs/2026-10-11-workflow-w7b-executions-design.md`
 * §12, FE-TEST-2).
 *
 * Opt-in (`EGRET_E2E_WORKFLOWS=1`). Like `workflows-flows.spec.ts` it needs a
 * backend with the workflow worker on and internal storage (the `Write` task
 * stores a file), signs in once as the break-glass local admin, and uses only
 * core tasks: default settings allow no container image.
 *
 * HOW TO RUN IT, on a stack of its own and never the shared dev stack (no
 * Postgres :5432, no shared bucket, no kind cluster). From the egret repo root:
 *
 * ```bash
 * # 1. A database of its own on the branch's Postgres (:5442), a private RustFS with one bucket.
 * docker exec egret-w7b-postgres psql -U egret -d postgres -c "CREATE DATABASE egret_w7b_e2e"
 * docker run -d --name w7b-t7-rustfs -p 9457:9000 \
 *   -e RUSTFS_ACCESS_KEY=egret -e RUSTFS_SECRET_KEY=egretsecret rustfs/rustfs:1.0.1
 * curl --aws-sigv4 "aws:amz:us-east-1:s3" --user egret:egretsecret -X PUT http://localhost:9457/warehouse
 *
 * # 2. The backend on :8197, pointed only at those two.
 * export DATABASE_URL=postgres://egret:egret@localhost:5442/egret_w7b_e2e
 * export EGRET_DATABASE__URL=$DATABASE_URL EGRET_SERVER__PORT=8197 EGRET_SERVER__HOST=127.0.0.1
 * export EGRET_OIDC__ENABLED=false EGRET_WORKFLOW__WORKER_ENABLED=true
 * export EGRET_STORAGE__ENDPOINT=http://localhost:9457 EGRET_STORAGE__BUCKET=warehouse
 * export EGRET_STORAGE__ACCESS_KEY=egret EGRET_STORAGE__SECRET_KEY=egretsecret
 * # `init` generates a random password unless one is given; the spec signs in with this one.
 * export EGRET_ADMIN_PASSWORD='Admin12345!'
 * # Only while iterating, and only on this private backend: sign-in is limited to 5 per 5 minutes.
 * # export EGRET_RATE_LIMIT__LOGIN_MAX_ATTEMPTS=200
 * cargo build -p egret-bin
 * ./target/debug/egret migrate && ./target/debug/egret init   # admin@egret.local
 * ./target/debug/egret serve
 *
 * # 3. The frontend on :5207, proxying /api to :8197 (the committed config next to this spec).
 * cd frontend && EGRET_E2E_VITE_PORT=5207 EGRET_E2E_API=http://localhost:8197 \
 *   npx vite --config tests/e2e/vite.e2e.config.mts
 *
 * # 4. The spec.
 * cd frontend && EGRET_E2E_WORKFLOWS=1 EGRET_E2E_BASE_URL=http://localhost:5207 \
 *   npx playwright test workflows-executions
 *
 * # 5. Afterwards: stop both servers, `docker rm -f w7b-t7-rustfs`, and drop the database by its name:
 * docker exec egret-w7b-postgres psql -U egret -d postgres -c "DROP DATABASE egret_w7b_e2e"
 * ```
 */
const ENABLED = process.env.EGRET_E2E_WORKFLOWS === "1";

const RUN = Date.now().toString(36);
const FLOW_ID = `w7b-e2e-${RUN}`;
const NAMESPACE = "e2e.w7b";
/** Lines the log-heavy task writes. */
const CHATTY_LINES = 400;
/** The loop's values: three iterations, three child executions. */
const LOOP_VALUES = ["a", "b", "c"];

const chattyMessages = Array.from({ length: CHATTY_LINES }, (_, i) => `      - "chatty line ${String(i + 1).padStart(3, "0")}"`).join("\n");

/**
 * Run 0 `flaky` fails its first attempt and succeeds on the second
 * (`RETRY_FAILED_TASK`, the same run); run 1 `write` stores a CSV; run 2
 * `chatty` writes 400 lines; run 3 `each` spawns three iterations; run 4
 * `wait` pauses for five minutes, which the spec cuts short with Resume; run
 * 5 `after` logs once the pause is released.
 */
const SOURCE = `id: ${FLOW_ID}
namespace: ${NAMESPACE}
tasks:
  - id: flaky
    type: io.kestra.plugin.core.execution.Fail
    condition: "{{ taskrun.attemptsCount == 0 }}"
    errorMessage: the first attempt fails on purpose
    retry:
      type: constant
      interval: PT1S
      maxAttempts: 3
  - id: write
    type: io.kestra.plugin.core.storage.Write
    content: "alpha,1\\nbeta,2\\ngamma,3\\n"
    extension: .csv
  - id: chatty
    type: io.kestra.plugin.core.log.Log
    message:
${chattyMessages}
  - id: each
    type: io.kestra.plugin.core.flow.Loop
    values: ${JSON.stringify(LOOP_VALUES)}
    tasks:
      - id: item
        type: io.kestra.plugin.core.log.Log
        message: "an iteration"
  - id: wait
    type: io.kestra.plugin.core.flow.Pause
    pauseDuration: PT5M
  - id: after
    type: io.kestra.plugin.core.log.Log
    message: after the pause
`;

/** The pause's deadline, which Resume must beat by far. */
const PAUSE_MS = 5 * 60_000;

const header = (page: Page): Locator => page.getByTestId("execution-summary");
const headerState = (page: Page, state: string): Locator =>
  page.locator("header", { has: page.getByTestId("execution-summary") }).locator(`[data-state="${state}"]`).first();

/** Two rectangles of the page, measured in one frame. */
async function rects(page: Page, selectors: string[]): Promise<(DOMRect | null)[]> {
  return page.evaluate((sels) => sels.map((s) => document.querySelector(s)?.getBoundingClientRect().toJSON() ?? null), selectors);
}

test.describe("W7b executions: list, gantt, logs, files, resume, replay", () => {
  test.skip(!ENABLED, "opt-in: set EGRET_E2E_WORKFLOWS=1 with a private local stack up (see the comment above)");
  test.setTimeout(240_000);

  test("roots then children → attempts in the gantt → a run's logs → preview → live logs → Resume → Replay", async ({ page, context }) => {
    // Every read of the logs route, to show that following stops once the log is complete.
    const logReads: number[] = [];
    page.on("request", (r) => {
      if (/\/workflows\/executions\/[^/]+\/logs\?/.test(r.url())) logReads.push(Date.now());
    });

    await loginLocalAdmin(page);
    const ws = (await firstWorkspace(page.request)).slug;
    const api = `/api/ws/${ws}/workflows`;
    const h = await csrf(context);

    // ── 0. The flow, created and started through the API (W7a drives the editor) ──
    const flow = await postJson(page.request, `${api}/flows`, h, { source: SOURCE });
    expect(flow.validation.errors, JSON.stringify(flow.validation)).toEqual([]);
    expect(flow.validation.warnings, JSON.stringify(flow.validation)).toEqual([]);
    const started = await postJson(page.request, `${api}/flows/${flow.id}/executions`, h, { inputs: {} });
    const execId: string = started.id;
    const startedAt = Date.now();
    const base = `/explore/${ws}/workflows`;
    const execHref = `${base}/executions/${execId}`;

    // ── 1. Logs while it runs: the lines arrive without a reload ────────
    await page.goto(`${execHref}/logs`);
    const logView = page.getByRole("log", { name: "Execution logs" });
    await expect(logView).toBeVisible({ timeout: 30_000 });
    // Followed to the bottom, the last chatty line is rendered; the group counts every line.
    await expect(logView.getByText(`chatty line ${CHATTY_LINES}`, { exact: true })).toBeVisible({ timeout: 60_000 });
    // Virtualised: 400 lines, a window of them in the DOM.
    const rendered = await logView.getByTestId("log-line").count();
    expect(rendered).toBeGreaterThan(5);
    expect(rendered).toBeLessThan(CHATTY_LINES);
    // Its group, 400 lines above the bottom, is out of the window; collapsed, every group is in view with its count.
    await page.getByRole("button", { name: "Collapse all" }).click();
    await expect(logView.getByTestId("log-group").filter({ hasText: "chatty #2" })).toContainText(`${CHATTY_LINES} lines`);
    await expect(logView.getByTestId("log-group").filter({ hasText: "flaky #0" })).toContainText("2 lines");
    await page.getByRole("button", { name: "Expand all" }).click();
    // The log fills the tab: its panel does not scroll around it (a fixed `100vh - 22rem` once did).
    const panelFit = await logView.evaluate((el) => {
      const panel = el.closest('[role="tabpanel"]')!;
      return { overflow: panel.scrollHeight - panel.clientHeight, gap: panel.getBoundingClientRect().bottom - el.getBoundingClientRect().bottom };
    });
    expect(panelFit.overflow, "the Logs tab's panel scrolls").toBeLessThanOrEqual(0);
    expect(panelFit.gap).toBeGreaterThanOrEqual(0);
    await expect(headerState(page, "PAUSED")).toBeVisible({ timeout: 60_000 });
    // Paused, the log is not complete: it is still being read.
    const readsWhilePaused = logReads.length;
    await expect.poll(() => logReads.length, { timeout: 10_000 }).toBeGreaterThan(readsWhilePaused + 1);

    // ── 2. The list: the root alone, then its three iterations ─────────
    await page.goto(`${base}/executions?flow=${flow.id}`);
    const table = page.getByTestId("executions-table");
    await expect(table.locator("tbody tr")).toHaveCount(1);
    await expect(table.locator("tbody tr").first()).toContainText(execId.slice(0, 8));
    await page.getByRole("switch", { name: "Show child executions" }).click();
    await expect(page).toHaveURL(/child=1/);
    await expect(table.locator("tbody tr")).toHaveCount(1 + LOOP_VALUES.length);
    await page.getByRole("switch", { name: "Show child executions" }).click();
    await expect(table.locator("tbody tr")).toHaveCount(1);
    // The flow's own Executions tab: the same root.
    await page.goto(`${base}/flows/${flow.id}/executions`);
    await expect(page.getByTestId("executions-table").locator("tbody tr")).toHaveCount(1);

    // ── 3. The gantt: two attempt segments on the retried run, measured ─
    await page.goto(`${execHref}/gantt`);
    const flaky = page.locator('[role="row"][data-run-ix="0"]');
    await expect(flaky).toContainText("2 attempts");
    await expect(page.getByTestId("gantt-attempt-0-0")).toHaveAttribute("data-state", "FAILED");
    await expect(page.getByTestId("gantt-attempt-0-1")).toHaveAttribute("data-state", "SUCCESS");
    await expect(page.getByTestId("gantt-attempt-0-2")).toHaveCount(0);
    const [a0, a1, cell] = await rects(page, [
      '[data-testid="gantt-attempt-0-0"]',
      '[data-testid="gantt-attempt-0-1"]',
      '[role="row"][data-run-ix="0"] > [role="gridcell"]:last-child',
    ]);
    expect(a0 && a1 && cell).toBeTruthy();
    // Both segments have width, sit inside the timeline cell, and the first ends where or before the second starts.
    expect(a0!.width).toBeGreaterThanOrEqual(1);
    expect(a1!.width).toBeGreaterThanOrEqual(1);
    for (const r of [a0!, a1!]) {
      expect(r.left).toBeGreaterThanOrEqual(cell!.left - 0.5);
      expect(r.right).toBeLessThanOrEqual(cell!.right + 0.5);
    }
    expect(a0!.right).toBeLessThanOrEqual(a1!.left + 1);
    // At 900 px every bar, attempt and marker stays inside its timeline cell, the chart does not
    // scroll sideways, and the retried row's task id is not truncated.
    await page.setViewportSize({ width: 900, height: 720 });
    const narrow = await page.evaluate(() => {
      const grid = document.querySelector('[role="treegrid"]')!;
      const outside: string[] = [];
      for (const row of grid.querySelectorAll('[role="row"][data-run-ix]')) {
        const cell = row.querySelector('[role="gridcell"]:last-child')!.getBoundingClientRect();
        for (const m of row.querySelectorAll<HTMLElement>('[data-testid^="gantt-"]')) {
          const r = m.getBoundingClientRect();
          if (r.left < cell.left - 0.5 || r.right > cell.right + 0.5) outside.push(m.dataset.testid!);
        }
      }
      const label = grid.querySelector('[role="row"][data-run-ix="0"] span.truncate')!;
      return { outside, sideways: grid.parentElement!.scrollWidth - grid.parentElement!.clientWidth, labelClipped: label.scrollWidth > label.clientWidth };
    });
    expect(narrow).toEqual({ outside: [], sideways: 0, labelClipped: false });
    await page.setViewportSize({ width: 1280, height: 720 });
    // The paused run's bar is still open: it runs to now.
    await expect(page.getByTestId("gantt-bar-4")).toHaveAttribute("data-open", "true");

    // Selecting the run opens its panel; its logs are its own lines only.
    await flaky.click();
    await expect(page).toHaveURL(/[?&]run=0/);
    const panel = page.getByTestId("gantt-run-panel");
    await expect(panel.getByRole("heading", { name: "flaky" })).toBeVisible();
    await expect(panel.getByRole("list", { name: "Attempts" }).getByRole("listitem")).toHaveCount(2);
    const runLog = panel.getByRole("log", { name: "Logs of flaky #0" });
    await expect(runLog.getByText("Fail on a condition")).toBeVisible();
    await expect(runLog.getByText(/chatty line/)).toHaveCount(0);
    // The attempt column appears because the run has two.
    await expect(runLog.getByTestId("log-attempt").first()).toHaveText("Attempt 1");
    // Keyboard: the selected row holds focus; Escape closes the panel, ArrowDown moves on.
    await flaky.focus();
    await page.keyboard.press("Escape");
    await expect(panel).toHaveCount(0);
    await page.keyboard.press("ArrowDown");
    await expect(page.locator('[role="row"][data-run-ix="1"]')).toBeFocused();
    await page.keyboard.press("Enter");
    await expect(page).toHaveURL(/[?&]run=1/);
    await expect(panel.getByRole("heading", { name: "write" })).toBeVisible();

    // The Logs tab's run filter does the same.
    await page.goto(`${execHref}/logs?run=2`);
    const filtered = page.getByRole("log", { name: "Execution logs" });
    await expect(filtered.getByText(`chatty line ${CHATTY_LINES}`, { exact: true })).toBeVisible();
    // Every rendered line is the run's own.
    const lines = await filtered.getByTestId("log-line").allTextContents();
    expect(lines.length).toBeGreaterThan(5);
    expect(lines.filter((t) => !/chatty line \d{3}$/.test(t))).toEqual([]);

    // ── 4. Outputs: the written file previews its rows ──────────────────
    await page.goto(`${execHref}/outputs?run=1`);
    const preview = page.getByRole("button", { name: "Preview content.csv" });
    await expect(preview).toBeVisible();
    await preview.click();
    const sheet = page.getByRole("dialog", { name: "Preview of content.csv" });
    await expect(sheet).toBeVisible();
    await expect(sheet.getByTestId("preview-row")).toHaveText(["alpha,1", "beta,2", "gamma,3"]);
    await page.keyboard.press("Escape");
    await expect(sheet).toBeHidden();
    await expect(preview).toBeFocused();

    // ── 5. Logs follow live through Resume, then stop ───────────────────
    await page.goto(`${execHref}/logs`);
    const live = page.getByRole("log", { name: "Execution logs" });
    await expect(live.getByText(`chatty line ${CHATTY_LINES}`, { exact: true })).toBeVisible();
    await expect(live.getByText("after the pause")).toHaveCount(0);
    await expect(headerState(page, "PAUSED")).toBeVisible();
    await page.getByRole("button", { name: "Resume", exact: true }).click();
    const resumeDialog = page.getByRole("alertdialog", { name: "Resume this execution?" });
    await resumeDialog.getByRole("button", { name: "Resume", exact: true }).click();
    await expect(resumeDialog).toBeHidden();
    await expect(page.getByText("Resumed the execution")).toBeVisible();
    await expect(headerState(page, "SUCCESS")).toBeVisible({ timeout: 60_000 });
    // Released long before its five minutes.
    expect(Date.now() - startedAt).toBeLessThan(PAUSE_MS / 2);
    // The line written after the pause arrives in the open view, without a reload.
    await expect(live.getByText("after the pause")).toBeVisible({ timeout: 30_000 });
    // Complete and read to the end, the view stops asking: no read in the next 6 s (it polls every 2 s).
    await page.waitForTimeout(3_000);
    const settled = logReads.length;
    await page.waitForTimeout(6_000);
    expect(logReads.length, "log reads after the log was complete").toBe(settled);

    // The gantt agrees: the pause ended SUCCESS and its bar is closed.
    await page.getByRole("tab", { name: "Gantt" }).click();
    await expect(page.locator('[role="row"][data-run-ix="4"] [data-state="SUCCESS"]').first()).toBeVisible();
    await expect(page.getByTestId("gantt-bar-4")).not.toHaveAttribute("data-open", "true");

    // ── 6. Replay: a new execution that says where it came from ─────────
    await page.getByRole("button", { name: "Replay", exact: true }).click();
    const replayDialog = page.getByRole("alertdialog", { name: "Replay this execution?" });
    await replayDialog.getByRole("button", { name: "Replay", exact: true }).click();
    await page.waitForURL((u) => /\/executions\/[0-9a-f-]{36}\/overview$/.test(u.pathname) && !u.pathname.includes(execId), {
      timeout: 15_000,
    });
    await expect(header(page)).toContainText("Replayed from");
    await expect(header(page).getByRole("link", { name: execId })).toBeVisible();
    await expect(page.getByRole("heading", { level: 1 })).not.toHaveText(execId);
  });
});
