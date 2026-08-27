import { test, expect, type Page, type Locator } from "@playwright/test";
import { loginLocalAdmin, firstWorkspace, csrf, postJson } from "./helpers";

/**
 * Regression for a layout defect no jsdom test can see: a MULTI-LINE refusal
 * rendered as one line.
 *
 * C2 gave the Data API editor a class of refusal it had never carried. Every
 * message these two alerts showed before was a sentence; a response transform's
 * is often the Starlark interpreter's own diagnostic, and that is an ASCII
 * diagram whose caret row means nothing unless it sits under the token it
 * points at:
 *
 *     error: Parse error: unexpected new line, expected symbol ':'
 *      --> transform.star:1:34
 *       |
 *     1 | def transform(params, statements)
 *       |
 *
 * Measured in Chrome at 1440px against this branch before the fix: the preview
 * alert's own `getComputedStyle(...).whiteSpace` was `normal`, its rendered text
 * occupied 2 line boxes for a message carrying 6 newlines, and the whole diagram
 * printed as `... | 1 | def transform(params, statements) | |` — every newline
 * and every run of alignment spaces collapsed. The publish dialog was worse: the
 * same collapse in a PROPORTIONAL font, so the caret row `^^^^^^^` ended up
 * beside the code instead of beneath it.
 *
 * **Why this is not a jsdom test.** `ResponseSection.test.tsx` and the rest of
 * the unit suite render into jsdom, which has no layout engine and does not
 * apply Tailwind at all: `getComputedStyle(el).whiteSpace` is the initial value
 * whatever classes the element carries, and `Range.getClientRects()` returns
 * nothing. A unit test asserting the message TEXT passes identically against
 * the broken and the fixed rendering — the string was always right; what was
 * wrong was that the browser threw the newlines away. See
 * `docs/en/ui-conventions.md` §10 and the sibling
 * `data-api-statement-strip-overlap.spec.ts`, which exists for the same reason.
 *
 * **Two assertions per site, deliberately.** The computed `white-space` pins the
 * property; the count of distinct rendered line boxes pins its consequence. The
 * second on its own would be satisfiable by wrapping (a long enough sentence
 * occupies several boxes with `white-space: normal` too), which is exactly why
 * it is expressed as "at least as many boxes as the message has lines" rather
 * than "more than one".
 *
 * **No engine is required.** Both refusals come from `check_transform`, which
 * `validate_preview` and `lifecycle::validate` (publish) each run BEFORE any SQL
 * is rendered or described — an unparseable script is refused without the
 * database connection ever being opened. The throwaway `databases` row below is
 * therefore never queried, matching the sibling spec's own note.
 *
 *   EGRET_E2E_DATAAPI_TRANSFORM=1 npx playwright test data-api-transform-refusal-legibility
 */
const ENABLED = process.env.EGRET_E2E_DATAAPI_TRANSFORM === "1";

/** Missing the `:` — `starlark`'s parser answers with the caret diagram above. */
const UNPARSEABLE = "def transform(params, statements)\n    return {}\n";

/**
 * How the message actually rendered: its computed `white-space`, the number of
 * distinct line-box TOPS its text occupies, and the number of lines the message
 * itself has.
 *
 * `Range.getClientRects()` over the element's contents, not `offsetHeight /
 * lineHeight`: the rects are the real line boxes the browser laid out, so a
 * changed font size or line height cannot turn a broken rendering into a
 * passing count. Tops are rounded and de-duplicated because a line box holding
 * more than one text run reports one rect per run.
 */
async function measure(alert: Locator) {
  return alert.evaluate((root) => {
    // `[role="alert"]` is the `Alert` WRAPPER; the classes under test sit on
    // the `AlertDescription` inside it, which has no test id of its own. Walk
    // down to the deepest element still holding the whole message — measuring
    // the wrapper reports the wrapper's own `white-space`, which is `normal`
    // whatever the description does, and would fail against the fix.
    let el = root as HTMLElement;
    for (;;) {
      const only = Array.from(el.children).find((c) => c.textContent === el.textContent);
      if (!only) break;
      el = only as HTMLElement;
    }
    const text = el.textContent ?? "";
    const range = document.createRange();
    range.selectNodeContents(el);
    const tops = new Set(
      Array.from(range.getClientRects())
        .filter((r) => r.width > 0 && r.height > 0)
        .map((r) => Math.round(r.top)),
    );
    return {
      text,
      whiteSpace: getComputedStyle(el).whiteSpace,
      fontFamily: getComputedStyle(el).fontFamily,
      renderedLines: tops.size,
      // Lines with something on them. A blank line lays out a line box of zero
      // WIDTH, which the filter above drops, and the diagnostics end with a
      // trailing newline — counting raw `split("\n").length` would demand one
      // more box than the browser can ever produce. Wrapping only ever ADDS
      // boxes, so `renderedLines >= messageLines` stays a lower bound.
      messageLines: text.split("\n").filter((l) => l.trim() !== "").length,
    };
  });
}

/** The one `AlertDescription` on screen carrying the refusal. */
function refusalAlert(page: Page, within?: Locator): Locator {
  const root = within ?? page.locator("main");
  return root.locator('[role="alert"]', { hasText: "the transform script" }).first();
}

test.describe("Data API — a transform's refusal keeps its line breaks", () => {
  test.skip(!ENABLED, "opt-in: set EGRET_E2E_DATAAPI_TRANSFORM=1 with the live dev stack up");
  test.setTimeout(120_000);

  test("preview and publish both render the Starlark diagnostic as the diagram it is", async ({
    page,
  }) => {
    await page.setViewportSize({ width: 1440, height: 900 });
    await loginLocalAdmin(page);
    const ws = await firstWorkspace(page.request);
    const h = await csrf(page.context());

    const runId = `${Date.now()}`;
    let dbId: string | undefined;
    let endpointId: string | undefined;

    try {
      // A real row in `databases`, never queried — see the header.
      const db = await postJson(page.request, `/api/ws/${ws.slug}/databases`, h, {
        name: `e2e-transform-${runId}`,
        engine: "starrocks",
        host: "localhost",
        port: 9030,
        options: { user: "root" },
      });
      dbId = db.id as string;

      // Saved with the bad script on purpose: Publish freezes what was last
      // SAVED, and create/update deliberately do not run `check_transform`
      // (a draft may be broken mid-edit), so this is a state an author can
      // really be in.
      const endpoint = await postJson(page.request, `/api/ws/${ws.slug}/data-api/endpoints`, h, {
        name: "E2E transform refusal legibility",
        path: `e2e-transform-refusal-${runId}`,
        database_id: dbId,
        statements: [{ name: "main", sql: "SELECT 1 AS n" }],
        output: {
          envelope: "wrapped",
          transform: { script: UNPARSEABLE, returns: [{ name: "n", ty: "integer" }] },
        },
      });
      endpointId = endpoint.id as string;

      // ── Preview ────────────────────────────────────────────────────────
      await page.goto(`/explore/${ws.slug}/data-api/endpoints/${endpointId}`);
      await page.getByRole("button", { name: "Preview unsaved draft" }).click();

      const previewAlert = refusalAlert(page);
      await expect(previewAlert).toBeVisible();
      const preview = await measure(previewAlert);

      expect(
        preview.text,
        "the server sends the parse diagram, or this spec is measuring the wrong message",
      ).toContain("transform.star:1:");
      expect(preview.messageLines, "the refusal really is multi-line").toBeGreaterThan(1);
      expect(
        preview.whiteSpace,
        `preview refusal must preserve its newlines, got white-space: ${preview.whiteSpace}`,
      ).toMatch(/^pre/);
      expect(
        preview.renderedLines,
        `preview refusal laid out on ${preview.renderedLines} line boxes for a ` +
          `${preview.messageLines}-line message`,
      ).toBeGreaterThanOrEqual(preview.messageLines);

      // ── Publish ────────────────────────────────────────────────────────
      await page.goto(`/explore/${ws.slug}/data-api/endpoints/${endpointId}/versions`);
      await page.getByRole("button", { name: "Publish saved draft" }).click();
      const dialog = page.getByRole("dialog");
      await expect(dialog).toBeVisible();
      await dialog.getByRole("button", { name: "Publish", exact: true }).click();

      const publishAlert = refusalAlert(page, dialog);
      await expect(publishAlert).toBeVisible();
      const publish = await measure(publishAlert);

      expect(publish.messageLines, "the refusal really is multi-line").toBeGreaterThan(1);
      expect(
        publish.whiteSpace,
        `publish refusal must preserve its newlines, got white-space: ${publish.whiteSpace}`,
      ).toMatch(/^pre/);
      expect(
        publish.renderedLines,
        `publish refusal laid out on ${publish.renderedLines} line boxes for a ` +
          `${publish.messageLines}-line message`,
      ).toBeGreaterThanOrEqual(publish.messageLines);
      // A caret row only lines up under the token it points at in a monospace
      // font — a diagram in a proportional one is legible-ish and still wrong.
      expect(
        publish.fontFamily.toLowerCase(),
        `a multi-line diagnostic must be monospace, got ${publish.fontFamily}`,
      ).toContain("mono");
    } finally {
      // Best-effort: a failed assertion above must not leave the shared
      // `default` workspace holding a throwaway endpoint or connection.
      if (endpointId) {
        await page.request
          .delete(`/api/ws/${ws.slug}/data-api/endpoints/${endpointId}`, { headers: h })
          .catch(() => {});
      }
      if (dbId) {
        await page.request.delete(`/api/ws/${ws.slug}/databases/${dbId}`, { headers: h }).catch(() => {});
      }
    }
  });
});
