import { test, expect } from "@playwright/test";

/**
 * The contrast measurements behind `ui/button.tsx`'s disabled treatment, taken
 * in a real browser because there is nowhere else they can be taken.
 *
 * # The defect
 *
 * Disabled used to be `disabled:opacity-50` and nothing else. Read off the
 * deployed build with `getComputedStyle`, on the Data API endpoint-detail Save
 * button, in dark mode:
 *
 *     disabled = true
 *     opacity  = 0.5
 *     background = oklch(0.66 0.13 185)   // unchanged from enabled
 *     color      = oklch(0.1 0 0)         // unchanged
 *
 * Alpha is not a colour change. It is a blend with whatever happens to be
 * behind, so one declaration produced two different results: blended into white
 * the teal washes out to a pastel and reads unavailable; blended into the
 * near-black dark ground it stays saturated and reads READY. In dark mode
 * nothing on screen said that Save does nothing.
 *
 * # Why a real browser
 *
 * jsdom computes no layout, paints nothing, and does not resolve `oklch()` — it
 * hands back the token string unevaluated (`docs/en/ui-conventions.md` §10). A
 * jsdom "contrast test" would therefore return the same answer with this fix
 * reverted. Everything below is resolved by Chrome itself: each computed colour
 * is painted into a 1x1 canvas over the surface it actually sits on and read
 * back as sRGB, which also composites the alpha tokens (`--border` is
 * `oklch(1 0 0 / 10%)` in dark) exactly as the compositor does.
 *
 * # What is asserted, and against which bar
 *
 * A disabled control is EXEMPT from WCAG 1.4.3 (4.5:1, text) and 1.4.11 (3:1,
 * non-text) — both carve out inactive components. So the bar here is not
 * "readable", it is DISTINGUISHABLE FROM ENABLED, which is a different property
 * and the one the defect broke. It is measured as the contrast between the
 * disabled control's painted fill and the same variant's enabled fill, and the
 * 3:1 of 1.4.11 is borrowed as a reference point rather than claimed as a
 * requirement. Luminance ratio is blind to hue, so the chroma of both fills is
 * asserted too: the whole point of "lose the fill" is that the teal goes away,
 * and a ratio alone cannot see that it did.
 *
 * The label's contrast on the disabled fill is asserted at the 4.5:1 text bar
 * even though it is exempt, because it costs nothing here and a disabled button
 * whose label cannot be read is a worse control than one that can.
 *
 * `default` is the variant asserted numerically: it is the Save / Create
 * endpoint button the audit measured, and it is the only variant with a strong
 * fill in BOTH themes. `secondary` and `outline` are deliberately not held to
 * the fill number — their enabled state has essentially no fill to lose, so
 * their state cue is the label ink and the inset ring, and pretending otherwise
 * would put a number in this file that means nothing.
 *
 * # Prerequisites
 *
 * The Vite dev server only (`npm run dev`) — no backend, no login, no
 * workspace. The page is used purely as a host for the compiled stylesheet; the
 * probes are built from the real module, so `buttonVariants` and `cn` are the
 * ones the product ships rather than a copy of their class strings.
 *
 *   EGRET_E2E_DISABLED_PAINT=1 npx playwright test disabled-button-paint
 */
const ENABLED = process.env.EGRET_E2E_DISABLED_PAINT === "1";

/** WCAG 1.4.11 Non-text Contrast, borrowed as a reference point — see above. */
const STATE_DIFFERENCE_REFERENCE = 3;
/** WCAG 1.4.3 Contrast (Minimum). The disabled label clears it although exempt. */
const TEXT_CONTRAST_MINIMUM = 4.5;
/** OKLab chroma below which a fill is neutral grey rather than a brand colour. */
const NEUTRAL_CHROMA = 0.02;

type Reading = {
  fill: string;
  ink: string;
  fillChroma: number;
  labelOnFill: number;
  stateFillContrast: number;
  fillVsSurface: number;
  widthShift: number;
  heightShift: number;
  hasInsetRing: boolean;
  disabledOpacity: number;
};

test.describe("A disabled button is painted disabled, in both themes", () => {
  test.skip(!ENABLED, "opt-in: set EGRET_E2E_DISABLED_PAINT=1 with `npm run dev` up");

  for (const theme of ["light", "dark"] as const) {
    test(`${theme} mode: the primary loses its fill rather than its alpha`, async ({ page }) => {
      await page.goto("/login");

      const readings = await page.evaluate(async (mode) => {
        const mod = await import("/src/components/ui/button.tsx");
        const utils = await import("/src/lib/utils.ts");
        const { buttonVariants } = mod as { buttonVariants: (o: { variant: string }) => string };
        const { cn } = utils as { cn: (...c: string[]) => string };

        // The theme lives on <html>, and the `dark` variant is `&:is(.dark *)`,
        // so a "light" probe nested inside a dark document would silently be a
        // dark probe. Set it at the root and measure one theme per test.
        document.documentElement.classList.toggle("dark", mode === "dark");

        const host = document.createElement("div");
        host.style.cssText = "position:fixed;left:0;top:0;z-index:99999";
        const pane = document.createElement("div");
        pane.className = "bg-card p-4 flex flex-col gap-2";
        const variants = ["default", "destructive", "outline", "secondary", "ghost", "link", "ai"];
        for (const v of variants) {
          for (const state of ["enabled", "disabled"]) {
            const b = document.createElement("button");
            b.className = cn(buttonVariants({ variant: v }));
            b.id = `probe-${v}-${state}`;
            b.textContent = v;
            if (state === "disabled") b.disabled = true;
            pane.appendChild(b);
          }
        }
        host.appendChild(pane);
        document.body.appendChild(host);
        await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));

        // Chrome's own colour resolution: paint the computed value over the
        // surface it sits on and read the pixel back. Nothing here parses an
        // `oklch(…)` string into three numbers — that mistake reads L, C and H
        // as R, G and B and reports confident garbage.
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
        const over = (fg: number[], alpha: number, bg: number[]) =>
          fg.map((c, i) => Math.round(c * alpha + bg[i] * (1 - alpha)));
        const luminance = (c: number[]) => {
          const f = (v: number) => {
            const u = v / 255;
            return u <= 0.04045 ? u / 12.92 : Math.pow((u + 0.055) / 1.055, 2.4);
          };
          return 0.2126 * f(c[0]) + 0.7152 * f(c[1]) + 0.0722 * f(c[2]);
        };
        const ratio = (a: number[], b: number[]) => {
          const [x, y] = [luminance(a), luminance(b)];
          return Math.round(((Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05)) * 100) / 100;
        };
        const chroma = (c: number[]) => {
          const f = (v: number) => {
            const u = v / 255;
            return u <= 0.04045 ? u / 12.92 : Math.pow((u + 0.055) / 1.055, 2.4);
          };
          const [r, g, b] = [f(c[0]), f(c[1]), f(c[2])];
          const l = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b);
          const m = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b);
          const s = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b);
          const A = 1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s;
          const B = 0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s;
          return Math.round(Math.hypot(A, B) * 1000) / 1000;
        };
        const hex = (c: number[]) => "#" + c.map((v) => v.toString(16).padStart(2, "0")).join("");

        const surface = paint(getComputedStyle(pane).backgroundColor, [255, 255, 255]);
        const read = (id: string) => {
          const el = document.getElementById(id)!;
          const cs = getComputedStyle(el);
          const opacity = parseFloat(cs.opacity);
          const fillOnSurface = paint(cs.backgroundColor, surface);
          const box = el.getBoundingClientRect();
          return {
            // `opacity` applies to the element as a group, so the ink is
            // composited onto its own fill first and the group alpha applied to
            // the pair — which is what the compositor does.
            fill: over(fillOnSurface, opacity, surface),
            ink: over(paint(cs.color, fillOnSurface), opacity, surface),
            opacity,
            width: box.width,
            height: box.height,
            insetRing: cs.boxShadow.includes("inset"),
          };
        };

        const out: Record<string, unknown> = {};
        for (const v of variants) {
          const en = read(`probe-${v}-enabled`);
          const di = read(`probe-${v}-disabled`);
          out[v] = {
            fill: hex(di.fill),
            ink: hex(di.ink),
            fillChroma: chroma(di.fill),
            enabledFillChroma: chroma(en.fill),
            labelOnFill: ratio(di.ink, di.fill),
            stateFillContrast: ratio(di.fill, en.fill),
            stateInkContrast: ratio(di.ink, en.ink),
            fillVsSurface: ratio(di.fill, surface),
            widthShift: Math.round((di.width - en.width) * 100) / 100,
            heightShift: Math.round((di.height - en.height) * 100) / 100,
            hasInsetRing: di.insetRing,
            disabledOpacity: di.opacity,
          };
        }
        host.remove();
        return out as Record<string, Reading & { enabledFillChroma: number; stateInkContrast: number }>;
      }, theme);

      const primary = readings.default;

      // ── The defect, inverted ────────────────────────────────────────────────
      expect(
        primary.stateFillContrast,
        `disabled primary fill ${primary.fill} against the enabled teal (light 3.10, dark 5.24 when written)`,
      ).toBeGreaterThanOrEqual(STATE_DIFFERENCE_REFERENCE);

      // A ratio cannot see a hue change, and "loses its fill" is a hue claim.
      // Under the old `opacity-50` the disabled chroma was 0.077 in light and
      // 0.074 in dark — still unmistakably teal in both.
      expect(readings.default.enabledFillChroma).toBeGreaterThan(0.05);
      expect(
        primary.fillChroma,
        "the disabled fill is a neutral, not a diluted brand colour",
      ).toBeLessThan(NEUTRAL_CHROMA);

      // ── The state must not be carried by alpha ─────────────────────────────
      expect(primary.disabledOpacity, "the filled variants no longer fade").toBe(1);

      // ── It must still read as a control ────────────────────────────────────
      expect(primary.hasInsetRing, "the disabled fill is bounded by a token border").toBe(true);
      expect(primary.labelOnFill).toBeGreaterThanOrEqual(TEXT_CONTRAST_MINIMUM);

      // ── And it must not move ───────────────────────────────────────────────
      // The border is an inset ring for this reason: a real 1px border would add
      // 2px of width, and these buttons toggle disabled as a form goes dirty and
      // clean, so Save would twitch on every keystroke.
      expect(primary.widthShift, "disabling changes no geometry").toBe(0);
      expect(primary.heightShift).toBe(0);

      // ── The variants that also carry a fill ────────────────────────────────
      for (const variant of ["destructive", "ai"] as const) {
        const r = readings[variant];
        expect(r.fill, `${variant} lands on the same disabled fill as the primary`).toBe(primary.fill);
        expect(r.fillChroma, `${variant} disabled is neutral`).toBeLessThan(NEUTRAL_CHROMA);
        expect(r.disabledOpacity).toBe(1);
      }

      // ── The borderless pair keeps its transparent ground ───────────────────
      // Filling these would be a regression, not an improvement: in dark mode
      // `--secondary` and `--accent` are the same value, and `--accent` is what
      // ghost paints on HOVER, so a filled disabled ghost would be identical to
      // a hovered one.
      for (const variant of ["ghost", "link"] as const) {
        const r = readings[variant];
        expect(r.fillVsSurface, `${variant} disabled adds no box`).toBe(1);
        expect(r.hasInsetRing).toBe(false);
        expect(r.stateInkContrast, `${variant} changes its ink instead`).toBeGreaterThan(1);
      }
    });
  }

  test("dark mode: the disabled paint survives a hover, and the button is still hoverable", async ({
    page,
  }) => {
    await page.goto("/login");
    await page.evaluate(async () => {
      const mod = await import("/src/components/ui/button.tsx");
      const utils = await import("/src/lib/utils.ts");
      const { buttonVariants } = mod as { buttonVariants: (o: { variant: string }) => string };
      const { cn } = utils as { cn: (...c: string[]) => string };
      document.documentElement.classList.add("dark");
      const host = document.createElement("div");
      host.style.cssText = "position:fixed;left:0;top:0;z-index:99999";
      const b = document.createElement("button");
      b.className = cn(buttonVariants({ variant: "default" }));
      b.id = "probe-hover";
      b.textContent = "Save";
      b.disabled = true;
      b.title = "Save your changes first";
      host.appendChild(b);
      document.body.appendChild(host);
    });

    const probe = page.locator("#probe-hover");
    const atRest = await probe.evaluate((el) => getComputedStyle(el).backgroundColor);

    // A real hover. It is the assertion as much as the setup: `pointer-events-none`
    // was removed from this component on purpose so a disabled control can still
    // be pointed at for its `title`, and reintroducing it would make this line
    // time out with "intercepts pointer events" rather than fail an expectation.
    await probe.hover();

    const hovered = await probe.evaluate((el) => ({
      matchesHover: el.matches(":hover"),
      background: getComputedStyle(el).backgroundColor,
      cursor: getComputedStyle(el).cursor,
      deepest: (document.querySelectorAll(":hover")[document.querySelectorAll(":hover").length - 1] as HTMLElement)?.id,
    }));

    expect(hovered.deepest, "the disabled button is the element under the pointer").toBe("probe-hover");
    expect(hovered.matchesHover).toBe(true);
    expect(hovered.cursor).toBe("not-allowed");
    // The variant's `hover:bg-primary/90` matches a disabled button too, because
    // hit-testing is what decides `:hover`. What this catches, measured by
    // mutation: with the disabled FILL removed (back to `opacity-50` alone) this
    // line reports `oklab(0.66 -0.129505 -0.0113303)` — the hover teal — against
    // a resting value that was the plain teal. Removing only the
    // `disabled:hover:*` twin does NOT fail it, because `disabled:` is emitted
    // after `hover:` at equal specificity; the twin is insurance against that
    // order, and this assertion is honest about which of the two it pins.
    expect(hovered.background, "hovering a disabled button does not repaint it").toBe(atRest);
  });
});
