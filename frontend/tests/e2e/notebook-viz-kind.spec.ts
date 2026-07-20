import { test, expect, type Page } from "@playwright/test";
import { loginLocalAdmin } from "./helpers";

/**
 * Notebook visualization e2e against the IN-CLUSTER kind deploy (egret.localtest.me).
 *
 * Proves the two interactive-viz money paths render LIVE on a real per-user kernel:
 *   - a plotly figure (emitted as `application/vnd.plotly.v1+json`) renders inside
 *     the FE `<div data-testid="notebook-plotly">` via the lazy plotly.js bundle,
 *   - a matplotlib figure (emitted as `image/png`) renders as an inline `<img>`.
 *
 * Opt-in: set EGRET_E2E_KIND=1 (needs the in-cluster kind deploy w/ notebook
 * kernels). NOT part of `npm test` / GitHub CI. Run via `just e2e-kind-notebook-viz`.
 */
const ENABLED = process.env.EGRET_E2E_KIND === "1";

/** Set the Nth mounted Monaco editor's value (typing into Monaco is flaky). */
async function setCellSource(page: Page, index: number, code: string) {
  await page.waitForFunction(
    (i) => (window as any).monaco?.editor?.getEditors?.().length > i,
    index,
    { timeout: 20_000 },
  );
  await page.evaluate(
    ({ i, src }) => (window as any).monaco.editor.getEditors()[i].setValue(src),
    { i: index, src: code },
  );
}

async function newNotebook(page: Page, name: string) {
  await page.getByRole("link", { name: "Notebooks" }).click();
  await page.getByRole("button", { name: "+ Notebook" }).click();
  await page.getByRole("textbox", { name: "Name" }).fill(name);
  await page.getByRole("button", { name: "Create", exact: true }).click();
  await page.waitForURL(/\/notebooks\/[0-9a-f-]{36}$/, { timeout: 20_000 });
}

test.describe("notebook visualization on kind", () => {
  test.skip(!ENABLED, "opt-in: set EGRET_E2E_KIND=1 against the in-cluster kind deploy");
  // First run cold-starts a per-user kernel pod (image pull + jupyter boot); the
  // two viz tests must not race a shared kernel, so run them serially.
  test.describe.configure({ mode: "serial" });
  test.setTimeout(300_000);

  test("a plotly cell renders an interactive chart", async ({ page }) => {
    await loginLocalAdmin(page);
    await newNotebook(page, `viz-plotly ${Date.now()}`);

    // The bare `fig` as the last expression triggers the mimetype display
    // (application/vnd.plotly.v1+json), which the FE routes to notebook-plotly.
    await setCellSource(
      page,
      0,
      [
        "import plotly.express as px",
        'fig = px.bar(x=["a", "b", "c"], y=[1, 2, 3])',
        "fig",
      ].join("\n"),
    );
    await page.getByRole("button", { name: "Run all" }).click();

    // First run cold-starts the per-user kernel pod + lazy-loads plotly.js → wait.
    await expect(page.getByTestId("notebook-plotly")).toBeVisible({ timeout: 150_000 });
    // plotly.js renders a `.plotly` container / svg inside — assert it drew.
    await expect(
      page
        .locator('[data-testid="notebook-plotly"] .plotly, [data-testid="notebook-plotly"] svg')
        .first(),
    ).toBeVisible({ timeout: 30_000 });
  });

  test("a matplotlib cell renders an image", async ({ page }) => {
    await loginLocalAdmin(page);
    await newNotebook(page, `viz-mpl ${Date.now()}`);

    await setCellSource(
      page,
      0,
      [
        "import matplotlib",
        "import matplotlib.pyplot as plt",
        "fig, ax = plt.subplots()",
        "ax.plot([1, 2, 3], [2, 4, 6])",
        "plt.show()",
      ].join("\n"),
    );
    await page.getByRole("button", { name: "Run all" }).click();

    // matplotlib emits image/png → the FE renders it as an inline data-URI <img>.
    await expect(page.locator("img[src^='data:image/png']").first()).toBeVisible({
      timeout: 150_000,
    });
  });
});
