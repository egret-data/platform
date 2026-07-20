import { test, expect, type Page } from "@playwright/test";
import { loginLocalAdmin } from "./helpers";

/**
 * "Metabase ecommerce dashboard" notebook e2e — the ad-hoc-analysis money path.
 *
 * Builds a self-contained ecommerce dataset (a tiny Products/Orders model, à la
 * Metabase's sample store) entirely inside the kernel with Daft — no engine or
 * warehouse needed — then runs a realistic dashboard analysis and asserts the
 * numbers. Exercises, end-to-end on a REAL per-user kernel:
 *   - Python execution + `daft.from_pydict`
 *   - `daft.sql(...)` over bound DataFrames (JOIN / SUM / GROUP BY / ORDER BY)
 *   - `display()` table rendering
 *   - output persistence across reload (E-P2 recorder)
 *   - the `%%egret_sql --target daft` cell magic (Daft-local SQL cell)
 *
 * The figures are hand-computed and deterministic:
 *   revenue = Σ price·qty = 255.0 · AOV = 255/12 = 21.25 · customers = 6
 *   top product = Gadget (125) · by category: Gizmos 195, Gadgets 60
 *
 * Opt-in: set EGRET_E2E_KIND=1 (needs the in-cluster kind deploy w/ notebook
 * kernels). NOT part of `npm test` / GitHub CI. Run via `just e2e-kind`.
 */
const ENABLED = process.env.EGRET_E2E_KIND === "1";

/** The ecommerce dataset + dashboard analysis, as one self-contained cell. */
const DASHBOARD_CELL = [
  "import daft",
  "# --- Sample ecommerce store (Products + Orders), Metabase-style ---",
  "products = daft.from_pydict({",
  "    'product_id': [1, 2, 3],",
  "    'product': ['Widget', 'Gadget', 'Doohickey'],",
  "    'category': ['Gizmos', 'Gizmos', 'Gadgets'],",
  "    'price': [10.0, 25.0, 5.0],",
  "})",
  "order_ids = list(range(1, 13))",
  "customer_ids = [1, 2, 3, 1, 2, 3, 4, 5, 6, 4, 5, 6]",
  "orders = daft.from_pydict({",
  "    'order_id': order_ids,",
  "    'product_id': [1, 2, 3, 1, 2, 3, 1, 2, 3, 1, 2, 3],",
  "    'customer_id': customer_ids,",
  "    'qty': [2, 1, 4, 1, 2, 1, 3, 1, 2, 1, 1, 5],",
  "    'order_month': ['2026-01'] * 4 + ['2026-02'] * 4 + ['2026-03'] * 4,",
  "})",
  "# --- Revenue line items (join orders to product prices) ---",
  "rev = daft.sql(",
  "    'SELECT o.customer_id, o.qty, o.order_month, p.product, p.category, '",
  "    '(p.price * o.qty) AS revenue '",
  "    'FROM orders o JOIN products p ON o.product_id = p.product_id',",
  "    orders=orders, products=products,",
  ")",
  "# --- Dashboard KPIs ---",
  "total = daft.sql('SELECT SUM(revenue) AS t FROM rev', rev=rev).to_pydict()['t'][0]",
  "top = daft.sql('SELECT product, SUM(revenue) AS r FROM rev GROUP BY product ORDER BY r DESC LIMIT 1', rev=rev).to_pydict()",
  "print('EGRET_TOTAL_REVENUE=' + str(round(float(total), 1)))",
  "print('EGRET_AOV=' + str(round(float(total) / len(order_ids), 2)))",
  "print('EGRET_CUSTOMERS=' + str(len(set(customer_ids))))",
  "print('EGRET_TOP_PRODUCT=' + str(top['product'][0]))",
  "# --- Dashboard charts (rendered as Daft tables) ---",
  "display(daft.sql('SELECT category, SUM(revenue) AS revenue FROM rev GROUP BY category ORDER BY revenue DESC', rev=rev))",
  "display(daft.sql('SELECT order_month, SUM(revenue) AS revenue FROM rev GROUP BY order_month ORDER BY order_month', rev=rev))",
].join("\n");

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

test.describe("ecommerce dashboard notebook on kind", () => {
  test.skip(!ENABLED, "opt-in: set EGRET_E2E_KIND=1 against the in-cluster kind deploy");
  // First run cold-starts a per-user kernel pod (image pull + jupyter boot).
  test.setTimeout(300_000);

  test("builds + runs an ecommerce dashboard (Daft + daft.sql + display), persisted on reload", async ({
    page,
  }) => {
    await loginLocalAdmin(page);
    await newNotebook(page, `Ecommerce Dashboard ${Date.now()}`);

    await setCellSource(page, 0, DASHBOARD_CELL);
    await page.getByRole("button", { name: "Run all" }).click();

    // KPIs (first run also cold-starts the per-user kernel pod → generous wait).
    await expect(page.getByText("EGRET_TOTAL_REVENUE=255.0")).toBeVisible({ timeout: 150_000 });
    await expect(page.getByText("EGRET_AOV=21.25")).toBeVisible();
    await expect(page.getByText("EGRET_CUSTOMERS=6")).toBeVisible();
    await expect(page.getByText("EGRET_TOP_PRODUCT=Gadget")).toBeVisible();
    // The by-category "chart" is a materialized Daft table: its revenue figures
    // (Gizmos 195, Gadgets 60) appear ONLY in the rendered output, never in the
    // cell source — so these prove the display() table actually rendered rows.
    await expect(page.getByText("195", { exact: false }).first()).toBeVisible();
    await expect(page.getByText("60", { exact: false }).first()).toBeVisible();

    // Output persists (E-P2 recorder): reload restores the KPIs from last_output.
    await page.reload();
    await expect(page.getByText("EGRET_TOTAL_REVENUE=255.0")).toBeVisible({ timeout: 30_000 });
    await expect(page.getByText("EGRET_TOP_PRODUCT=Gadget")).toBeVisible();
  });

  test("a Daft-local SQL cell computes revenue by category over bound DataFrames", async ({
    page,
  }) => {
    await loginLocalAdmin(page);
    await newNotebook(page, `Ecommerce SQL ${Date.now()}`);

    // Cell 1: generate + bind the `orders`/`products` DataFrames in the kernel.
    await setCellSource(
      page,
      0,
      [
        "import daft",
        "products = daft.from_pydict({'product_id':[1,2,3],'product':['Widget','Gadget','Doohickey'],'category':['Gizmos','Gizmos','Gadgets'],'price':[10.0,25.0,5.0]})",
        "orders = daft.from_pydict({'product_id':[1,2,3,1,2,3,1,2,3,1,2,3],'qty':[2,1,4,1,2,1,3,1,2,1,1,5]})",
        "print('EGRET_DATA_READY')",
      ].join("\n"),
    );
    await page.getByRole("button", { name: "Run all" }).click();
    await expect(page.getByText("EGRET_DATA_READY")).toBeVisible({ timeout: 150_000 });

    // Add a SQL cell (its target defaults to empty on an engine-less workspace).
    await page.getByRole("button", { name: "Add SQL cell" }).click();
    // Point it at the Daft-local target and confirm the selection committed.
    await page.getByLabel("SQL target").click();
    await page.getByRole("option", { name: "Daft (local)" }).click();
    await expect(page.getByLabel("SQL target")).toHaveText(/Daft \(local\)/);

    // Query over the DataFrames bound by cell 1 — set it into the SQL editor
    // (found by language, not position) and confirm the text committed.
    const sql =
      "SELECT p.category, SUM(p.price * o.qty) AS revenue FROM orders o JOIN products p ON o.product_id = p.product_id GROUP BY p.category ORDER BY revenue DESC";
    await page.evaluate((src) => {
      const eds = (window as any).monaco.editor.getEditors();
      const sqlEd = eds.find((e: any) => e.getModel()?.getLanguageId() === "sql");
      sqlEd.getModel().setValue(src);
    }, sql);
    await expect(page.getByText("GROUP BY p.category").first()).toBeVisible();

    // Run the SQL cell (2nd cell's Run) and assert the aggregation rendered:
    // revenue 195 (Gizmos) / 60 (Gadgets) appear only in the materialized result.
    await page.getByRole("button", { name: "Run", exact: true }).nth(1).click();
    await expect(page.getByText("195", { exact: false }).first()).toBeVisible({ timeout: 60_000 });
    await expect(page.getByText("60", { exact: false }).first()).toBeVisible();
  });
});
