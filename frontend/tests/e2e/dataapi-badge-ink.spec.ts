import { test, expect } from "@playwright/test";

/**
 * WHAT DATA API'S STATUS CHIPS ACTUALLY PAINT, measured in a real browser
 * because there is nowhere else they can be measured.
 *
 * # The defect
 *
 * Five Data API surfaces painted a status chip as coloured ink on a wash of its
 * own hue — `bg-success/15 text-success` for the endpoint list's `Live`, for
 * Versions' `Live`, for Clients' `Active`, for Activity's 2xx status code and
 * for Try It's outcome, with the `destructive` twin beside each. At the 12px
 * (`text-xs`) these chips render at, WCAG 2.2 1.4.3 asks 4.5:1, and the pairs
 * measure 3.72:1 and 3.68:1 in light mode. The endpoint list was the worst of
 * it: the SAME endpoint wore the failing chip in the table and, one click away
 * in the editor header, a pill that had already been fixed.
 *
 * The fix keeps the tint (it is the recognition cue the lists already trained,
 * and it is decoration — the word or the status code is what identifies the
 * state) and moves the ink to `--foreground`. See `src/routes/explore/dataapi/
 * chipTone.ts`.
 *
 * # Why a real browser, and why a canvas
 *
 * jsdom computes no layout, composites nothing, and does not resolve `oklch()`
 * — it hands the token string back unevaluated (`docs/en/ui-conventions.md`
 * §10). The unit guards in `Endpoints.test.tsx` and `EndpointHeader.test.tsx`
 * therefore assert only the WIRING: that each chip is handed `text-foreground`
 * and no hued ink. They would return the same answer with `--success` and
 * `--destructive` swapped, or with either deleted.
 *
 * Everything below is resolved by Chrome and read back as sRGB pixels: each
 * computed colour is painted over the surface it really sits on, so the `/15`
 * alpha is composited exactly as the compositor does it. Nothing here parses an
 * `oklch(…)` string into three numbers — that mistake reads L, C and H as R, G
 * and B and is confidently wrong in exactly the 2.2–2.6 band the real failures
 * live in.
 *
 * # Why the tones are IMPORTED and not retyped
 *
 * A spec that types `"bg-success/15 text-foreground"` into its own probe
 * measures the spec. `CHIP_TONE` and `TIER_TONE` are pulled out of the shipped
 * modules, so if a call site is rewritten back to a hued ink tomorrow, the
 * string this spec paints changes with it.
 *
 * # Why two surfaces
 *
 * The tint is 15% alpha, so what is UNDER it is part of the measurement.
 * `--card` and `--background` are the same colour in light mode and differ in
 * dark (0.21 vs 0.14), and these chips appear on both — Versions and Activity
 * inside a card, the endpoint list and Try It on the page. Both are required to
 * clear the bar; the weaker of the two is what a reader gets.
 *
 * # Controls
 *
 * Every run carries a probe that MUST pass and a probe that MUST fail, so a
 * green result cannot be the instrument returning nothing:
 *
 * - `--foreground` on the surface, which is body ink and reads ~18:1.
 * - The surface's own colour used as ink, which is 1.00:1 by construction. If
 *   this one ever clears 4.5:1 the harness is not measuring what it claims to.
 * - The RETIRED pair itself, `text-success` on `bg-success/15`, asserted to
 *   still fail in light. That is the defect; if it starts passing, either the
 *   tokens moved or the probe stopped compositing the wash.
 *
 * # Prerequisites
 *
 * The Vite dev server only (`npm run dev`) — no backend, no login, no
 * workspace. The page hosts the compiled stylesheet; the probes are built from
 * the real modules.
 *
 *   EGRET_E2E_BADGE_INK=1 npx playwright test dataapi-badge-ink
 */
const ENABLED = process.env.EGRET_E2E_BADGE_INK === "1";

/** WCAG 2.2 1.4.3 Contrast (Minimum), normal text. These chips are 12px. */
const TEXT_CONTRAST = 4.5;

/** The surfaces a Data API chip is ever laid on. */
const SURFACES = ["bg-card", "bg-background"] as const;

type Reading = { name: string; ratio: number; ink: string; fill: string };
type Measurement = { tones: Reading[]; controls: Record<string, Reading> };

async function measure(
  page: import("@playwright/test").Page,
  theme: "light" | "dark",
  surface: (typeof SURFACES)[number],
): Promise<Measurement> {
  return page.evaluate(
    async ({ mode, surfaceClass }) => {
      const chip = (await import("/src/routes/explore/dataapi/chipTone.ts")) as {
        CHIP_TONE: Record<string, string>;
      };
      const state = (await import("/src/routes/explore/dataapi/endpointState.tsx")) as {
        TIER_TONE: Record<string, string>;
      };

      // The theme lives on <html> and the `dark` variant is `&:is(.dark *)`, so
      // a "light" probe nested inside a dark document would silently be a dark
      // one.
      document.documentElement.classList.toggle("dark", mode === "dark");

      const host = document.createElement("div");
      host.style.cssText = "position:fixed;left:0;top:0;z-index:99999";
      const pane = document.createElement("div");
      pane.className = `${surfaceClass} p-4 flex flex-col gap-1`;

      // Every shipped tone, named by where it comes from. `TIER_TONE`'s values
      // are `CHIP_TONE`'s today; they are probed separately anyway, because the
      // day they stop being is the day this spec has to notice.
      const specs: [string, string][] = [
        ...Object.entries(chip.CHIP_TONE).map(([k, v]) => [`CHIP_TONE.${k}`, v] as [string, string]),
        ...Object.entries(state.TIER_TONE).map(([k, v]) => [`TIER_TONE.${k}`, v] as [string, string]),
      ];
      const controls: [string, string][] = [
        // Must pass: ordinary body ink on this surface.
        ["control-pass-body-ink", "text-foreground"],
        // Must fail: the retired pair, still measurable because both utilities
        // are still emitted elsewhere in the app.
        ["control-fail-retired-pair", "bg-success/15 text-success"],
      ];

      const mk = (id: string, cls: string) => {
        const span = document.createElement("span");
        span.id = `probe-${id}`;
        // The chip geometry `Badge` ships: 12px, medium, padded, rounded.
        span.className = `inline-flex items-center rounded-md border px-2 py-0.5 text-xs font-medium ${cls}`;
        span.textContent = "Live 200";
        pane.appendChild(span);
      };
      for (const [id, cls] of [...specs, ...controls]) mk(id, cls);

      // Must fail by construction: the surface's own colour as ink. Set through
      // `var(--token)` rather than a utility class, because a Tailwind utility
      // no source file uses is never emitted and would silently fall back to
      // the INHERITED colour — a control that quietly became a second copy of
      // the body-ink probe.
      const selfInk = document.createElement("span");
      selfInk.id = "probe-control-fail-self-ink";
      selfInk.className = "inline-flex items-center rounded-md px-2 py-0.5 text-xs font-medium";
      selfInk.style.color =
        surfaceClass === "bg-card" ? "var(--card)" : "var(--background)";
      selfInk.textContent = "Live 200";
      pane.appendChild(selfInk);

      host.appendChild(pane);
      document.body.appendChild(host);
      await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));

      // Chrome's own colour resolution: paint the computed value over the
      // surface it sits on and read the pixel back.
      const canvas = document.createElement("canvas");
      canvas.width = canvas.height = 1;
      const ctx = canvas.getContext("2d", { willReadFrequently: true })!;
      const paint = (color: string, backdrop: number[]) => {
        ctx.clearRect(0, 0, 1, 1);
        ctx.fillStyle = `rgb(${backdrop[0]},${backdrop[1]},${backdrop[2]})`;
        ctx.fillRect(0, 0, 1, 1);
        ctx.fillStyle = color;
        ctx.fillRect(0, 0, 1, 1);
        const d = ctx.getImageData(0, 0, 1, 1).data;
        return [d[0], d[1], d[2]];
      };
      const channel = (v: number) => {
        const u = v / 255;
        return u <= 0.04045 ? u / 12.92 : Math.pow((u + 0.055) / 1.055, 2.4);
      };
      const luminance = (c: number[]) =>
        0.2126 * channel(c[0]) + 0.7152 * channel(c[1]) + 0.0722 * channel(c[2]);
      const ratio = (a: number[], b: number[]) => {
        const [x, y] = [luminance(a), luminance(b)];
        return Math.round(((Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05)) * 100) / 100;
      };
      const hex = (c: number[]) => "#" + c.map((v) => v.toString(16).padStart(2, "0")).join("");

      const surfaceRgb = paint(getComputedStyle(pane).backgroundColor, [255, 255, 255]);
      const read = (id: string, name: string) => {
        const cs = getComputedStyle(document.getElementById(`probe-${id}`)!);
        // The wash is semi-transparent, so it is composited onto the surface
        // before the ink is measured against it. A chip with no fill of its own
        // reads the surface back unchanged, which is the correct backdrop.
        const fill = paint(cs.backgroundColor, surfaceRgb);
        const ink = paint(cs.color, fill);
        return { name, ratio: ratio(ink, fill), ink: hex(ink), fill: hex(fill) };
      };

      const out = {
        tones: specs.map(([id, ]) => read(id, id)),
        controls: {
          pass: read("control-pass-body-ink", "control-pass-body-ink"),
          failRetired: read("control-fail-retired-pair", "control-fail-retired-pair"),
          failSelfInk: read("control-fail-self-ink", "control-fail-self-ink"),
        },
      };
      host.remove();
      document.documentElement.classList.remove("dark");
      return out;
    },
    { mode: theme, surfaceClass: surface },
  );
}

test.describe("Data API status chips are legible, in both themes, on both surfaces", () => {
  test.skip(!ENABLED, "opt-in: set EGRET_E2E_BADGE_INK=1 with `npm run dev` up");

  for (const theme of ["light", "dark"] as const) {
    for (const surface of SURFACES) {
      test(`${theme} mode on ${surface}: every shipped chip tone clears WCAG 1.4.3`, async ({ page }) => {
        await page.goto("/login");
        const m = await measure(page, theme, surface);

        // CONTROLS FIRST. If the instrument is returning nothing, everything
        // below it is meaningless, and a "0 tones failed" over an empty list
        // reads exactly like a pass.
        expect(
          m.controls.pass.ratio,
          `control: body ink ${m.controls.pass.ink} on ${m.controls.pass.fill} must clear the bar`,
        ).toBeGreaterThanOrEqual(TEXT_CONTRAST);
        expect(
          m.controls.failSelfInk.ratio,
          `control: the surface's own colour as ink must NOT clear the bar — the probe is not compositing`,
        ).toBeLessThan(TEXT_CONTRAST);
        expect(
          m.tones.length,
          "control: no tones were probed at all, so nothing below was measured",
        ).toBeGreaterThanOrEqual(9);

        for (const t of m.tones) {
          expect(
            t.ratio,
            `${t.name}: ink ${t.ink} on ${t.fill} measures ${t.ratio}:1`,
          ).toBeGreaterThanOrEqual(TEXT_CONTRAST);
        }
      });
    }
  }

  // Split out and light-only: the retired pair passes in DARK (7.00 on the page
  // background), which is precisely why the light reading is the one that
  // matters and why a both-themes assertion here would have been wrong.
  test("light mode: the retired `text-success` on `bg-success/15` still fails, so the bar is real", async ({
    page,
  }) => {
    await page.goto("/login");
    const m = await measure(page, "light", "bg-background");
    expect(
      m.controls.failRetired.ratio,
      `the pair this change removed now measures ${m.controls.failRetired.ratio}:1 — if it passes, either the tokens moved or the wash is not being composited`,
    ).toBeLessThan(TEXT_CONTRAST);
  });
});
