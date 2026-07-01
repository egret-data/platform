import { test, expect, type Page } from "@playwright/test";
import { loginViaOidc } from "./helpers";

/**
 * Dataframe-first notebook e2e (opt-in — needs the live stack + a `notebook`
 * warehouse + at least one registered engine; NOT in CI). Verifies the money
 * path: an Iceberg write->read roundtrip (and, optionally, a governed engine read).
 */

async function setFirstCellSource(page: Page, code: string) {
  await page.waitForFunction(() => (window as any).monaco?.editor?.getEditors?.().length > 0, null, {
    timeout: 20_000,
  });
  await page.evaluate((src) => {
    (window as any).monaco.editor.getEditors()[0].setValue(src);
  }, code);
}

async function newNotebook(page: Page, name: string) {
  await page.getByRole("link", { name: "Notebooks" }).click();
  await page.getByRole("button", { name: "+ Notebook" }).click();
  await page.getByRole("textbox", { name: "Name" }).fill(name);
  await page.getByRole("button", { name: "Create", exact: true }).click();
  await page.waitForURL(/\/notebooks\/[0-9a-f-]{36}$/, { timeout: 20_000 });
}

test("iceberg write -> read roundtrip via Daft", async ({ page }) => {
  await loginViaOidc(page);
  await newNotebook(page, `df-e2e ${Date.now()}`);
  const marker = `egret-df-${Date.now()}`;
  await setFirstCellSource(
    page,
    [
      "import daft",
      `df = daft.from_pydict({"who": ["${marker}"], "n": [6 * 7]})`,
      'write_iceberg(df, "e2e_roundtrip", mode="overwrite")',
      'row = iceberg("e2e_roundtrip").to_pydict()',
      'print(row["who"][0], row["n"][0])',
    ].join("\n"),
  );
  await page.getByRole("button", { name: "Run", exact: true }).first().click();
  // `${marker} 42` appears ONLY in the printed roundtrip output — the source has
  // the marker inside `from_pydict` and `6 * 7`, never the adjacent literal — so
  // the assertion can't match the editor source (mirrors notebook.spec.ts).
  await expect(page.getByText(`${marker} 42`)).toBeVisible({ timeout: 60_000 });
});
