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
 * Alpha is not a colour change. It is a blend with whatever is behind, so one
 * declaration produced two results: blended into white the teal washes out to a
 * pastel and reads unavailable; blended into the near-black dark ground it stays
 * saturated and reads READY.
 *
 * # The second defect, which this file exists to make impossible
 *
 * The first fix gave EVERY variant the new paint, and for the variants whose
 * enabled fill is neutral that was a net regression: `outline` and `secondary`
 * went from 5.08:1 to 2.85:1 on the enabled-vs-disabled ink in light, and
 * 3.36:1 to 2.51:1 in dark, across 238 + 60 call sites. Alpha only preserves a
 * hue when there IS a hue; on a neutral fill `opacity-50` was never broken.
 *
 * So the assertions below are not fixed numbers. Each variant is measured three
 * ways in the same pass — enabled, disabled as it ships, and disabled as
 * `opacity-50` WOULD have painted it (the shipped class string with every
 * `disabled:`/`aria-disabled:` class stripped and `disabled:opacity-50` put
 * back) — and the shipped treatment must be at least as strong as that baseline
 * on every axis. A floor that recomputes itself cannot go stale, and it states
 * the rule directly: no variant may end up weaker than the alpha it replaced.
 *
 * # Why a real browser
 *
 * jsdom computes no layout, paints nothing, and does not resolve `oklch()` — it
 * hands back the token string unevaluated (`docs/en/ui-conventions.md` §10). A
 * jsdom "contrast test" would return the same answer with this fix reverted.
 * Everything below is resolved by Chrome: each computed colour is painted into a
 * 1x1 canvas over the surface it actually sits on and read back as sRGB, which
 * also composites the alpha tokens (`--border` is `oklch(1 0 0 / 10%)` in dark)
 * exactly as the compositor does.
 *
 * # Which bar
 *
 * A disabled control is EXEMPT from WCAG 1.4.3 (4.5:1, text) and 1.4.11 (3:1,
 * non-text) — both carve out inactive components. The bar here is
 * DISTINGUISHABLE FROM ENABLED, which is a different property and the one the
 * defect broke; 3:1 is borrowed as a reference point for the primary rather than
 * claimed as a requirement. Luminance ratio is blind to hue, so the chroma of
 * the primary's fill is asserted too: "loses its fill" is a hue claim and a
 * ratio cannot see it.
 *
 * # Prerequisites
 *
 * The Vite dev server only (`npm run dev`) — no backend, no login, no
 * workspace. The page hosts the compiled stylesheet; the probes are built from
 * the real module, so `buttonVariants` and `cn` are the ones the product ships.
 *
 *   EGRET_E2E_DISABLED_PAINT=1 npx playwright test disabled-button-paint
 */
const ENABLED = process.env.EGRET_E2E_DISABLED_PAINT === "1";

/** WCAG 1.4.11 Non-text Contrast, borrowed as a reference point — see above. */
const STATE_DIFFERENCE_REFERENCE = 3;
/** OKLab chroma below which a fill is neutral grey rather than a brand colour. */
const NEUTRAL_CHROMA = 0.02;

const VARIANTS = ["default", "destructive", "outline", "secondary", "ghost", "link", "ai"] as const;
type Variant = (typeof VARIANTS)[number];

type Axes = { ink: number; fill: number; label: number };
type Reading = {
  now: Axes;
  floor: Axes;
  fill: string;
  ink: string;
  fillChroma: number;
  enabledFillChroma: number;
  fillVsSurface: number;
  hasInsetRing: boolean;
  gradientGone: boolean;
  disabledOpacity: number;
  widthShift: number;
  heightShift: number;
};

async function measure(page: import("@playwright/test").Page, theme: "light" | "dark") {
  return page.evaluate(async (mode) => {
    const mod = await import("/src/components/ui/button.tsx");
    const utils = await import("/src/lib/utils.ts");
    const { buttonVariants } = mod as { buttonVariants: (o: { variant: string }) => string };
    const { cn } = utils as { cn: (...c: string[]) => string };

    // The theme lives on <html> and the `dark` variant is `&:is(.dark *)`, so a
    // "light" probe nested inside a dark document would silently be a dark one.
    document.documentElement.classList.toggle("dark", mode === "dark");

    // The pre-fix treatment, rebuilt from the shipped string rather than copied:
    // drop every disabled rule this component now sets and put the old alpha back.
    const asOpacity50 = (classes: string) =>
      classes
        .split(/\s+/)
        .filter((c) => !/(^|:)(aria-)?disabled:/.test(c))
        .concat("disabled:opacity-50")
        .join(" ");

    const host = document.createElement("div");
    host.style.cssText = "position:fixed;left:0;top:0;z-index:99999";
    const pane = document.createElement("div");
    pane.className = "bg-card p-4 flex flex-col gap-1";
    const variants = ["default", "destructive", "outline", "secondary", "ghost", "link", "ai"];
    const add = (id: string, className: string, disabled: boolean) => {
      const b = document.createElement("button");
      b.className = className;
      b.id = id;
      b.textContent = id;
      if (disabled) b.disabled = true;
      pane.appendChild(b);
    };
    for (const v of variants) {
      const shipped = cn(buttonVariants({ variant: v }));
      add(`enabled-${v}`, shipped, false);
      add(`now-${v}`, shipped, true);
      add(`floor-${v}`, asOpacity50(shipped), true);
    }
    host.appendChild(pane);
    document.body.appendChild(host);
    await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));

    // Chrome's own colour resolution: paint the computed value over the surface
    // it sits on and read the pixel back. Nothing here parses an `oklch(…)`
    // string into three numbers — that mistake reads L, C and H as R, G and B.
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
    const chroma = (c: number[]) => {
      const [r, g, b] = [channel(c[0]), channel(c[1]), channel(c[2])];
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
        // `opacity` applies to the element as a group, so the ink is composited
        // onto its own fill first and the group alpha applied to the pair.
        fill: over(fillOnSurface, opacity, surface),
        ink: over(paint(cs.color, fillOnSurface), opacity, surface),
        opacity,
        width: box.width,
        height: box.height,
        insetRing: cs.boxShadow.includes("inset"),
        gradient: cs.backgroundImage !== "none",
      };
    };

    const out: Record<string, unknown> = {};
    for (const v of variants) {
      const en = read(`enabled-${v}`);
      const now = read(`now-${v}`);
      const floor = read(`floor-${v}`);
      const axes = (d: ReturnType<typeof read>) => ({
        ink: ratio(d.ink, en.ink),
        fill: ratio(d.fill, en.fill),
        label: ratio(d.ink, d.fill),
      });
      out[v] = {
        now: axes(now),
        floor: axes(floor),
        fill: hex(now.fill),
        ink: hex(now.ink),
        fillChroma: chroma(now.fill),
        enabledFillChroma: chroma(en.fill),
        fillVsSurface: ratio(now.fill, surface),
        hasInsetRing: now.insetRing,
        gradientGone: !now.gradient,
        disabledOpacity: now.opacity,
        widthShift: Math.round((now.width - en.width) * 100) / 100,
        heightShift: Math.round((now.height - en.height) * 100) / 100,
      };
    }
    host.remove();
    return out as Record<Variant, Reading>;
  }, theme);
}

test.describe("A disabled button is painted disabled, in both themes", () => {
  test.skip(!ENABLED, "opt-in: set EGRET_E2E_DISABLED_PAINT=1 with `npm run dev` up");

  for (const theme of ["light", "dark"] as const) {
    test(`${theme} mode: no variant is weaker than the alpha it replaced`, async ({ page }) => {
      await page.goto("/login");
      const r = await measure(page, theme);

      // `ai` is excluded from the floor and only from the floor. Its enabled
      // fill is a `background-image`, and `getComputedStyle().backgroundColor`
      // cannot see a gradient — the baseline probe reports the surface colour,
      // so its three "old" numbers are measurements of nothing. Its gradient
      // removal is asserted separately below.
      for (const variant of VARIANTS.filter((v) => v !== "ai")) {
        const { now, floor } = r[variant];
        expect(now.ink, `${variant}: enabled-vs-disabled ink must not be weaker than opacity-50`).toBeGreaterThanOrEqual(floor.ink);
        expect(now.fill, `${variant}: enabled-vs-disabled fill must not be weaker than opacity-50`).toBeGreaterThanOrEqual(floor.fill);
        expect(now.label, `${variant}: the disabled label must not be less legible than under opacity-50`).toBeGreaterThanOrEqual(floor.label);
      }

      // And the floor must be a real measurement, not a row of 1s that anything
      // would clear: `outline`/`secondary`/`ghost` are the variants that KEEP
      // the alpha, so their floor is their shipped value exactly.
      for (const variant of ["outline", "secondary", "ghost"] as const) {
        expect(r[variant].floor.ink, `${variant} floor ink is a real number`).toBeGreaterThan(3);
        expect(r[variant].now.ink, `${variant} keeps the alpha cue unchanged`).toBe(r[variant].floor.ink);
        expect(r[variant].disabledOpacity).toBe(0.5);
        // Minor, fixed here: `outline` already has a border. Adding the inset
        // ring on top of it painted a doubled edge that existed only while
        // disabled.
        expect(r[variant].hasInsetRing, `${variant} does not gain a second edge`).toBe(false);
      }
    });

    test(`${theme} mode: the saturated variants lose their fill rather than their alpha`, async ({ page }) => {
      await page.goto("/login");
      const r = await measure(page, theme);
      const primary = r.default;

      // ── The original defect, inverted ──────────────────────────────────────
      expect(
        primary.now.fill,
        `disabled primary fill ${primary.fill} against the enabled teal (3.10 light, 5.24 dark when written)`,
      ).toBeGreaterThanOrEqual(STATE_DIFFERENCE_REFERENCE);
      expect(primary.floor.fill, "and the alpha it replaced was under that bar").toBeLessThan(
        STATE_DIFFERENCE_REFERENCE,
      );

      // A ratio cannot see a hue change, and "loses its fill" is a hue claim.
      // Under `opacity-50` the disabled chroma was 0.077 light / 0.074 dark —
      // still unmistakably teal in both.
      expect(primary.enabledFillChroma).toBeGreaterThan(0.05);
      expect(primary.fillChroma, "the disabled fill is a neutral, not a diluted brand colour").toBeLessThan(
        NEUTRAL_CHROMA,
      );

      // ── The state is not carried by alpha, and the box does not move ───────
      expect(primary.disabledOpacity, "the filled variants no longer fade").toBe(1);
      expect(primary.hasInsetRing, "the disabled fill is bounded by a token border").toBe(true);
      // The border is an inset ring for this reason: a real 1px border would add
      // 2px of width, and these buttons toggle disabled as a form goes dirty and
      // clean, so Save would twitch on every keystroke.
      expect(primary.widthShift, "disabling changes no geometry").toBe(0);
      expect(primary.heightShift).toBe(0);

      // ── The other two saturated variants land on the same paint ────────────
      for (const variant of ["destructive", "ai"] as const) {
        expect(r[variant].fill, `${variant} lands on the same disabled fill as the primary`).toBe(primary.fill);
        expect(r[variant].fillChroma, `${variant} disabled is neutral`).toBeLessThan(NEUTRAL_CHROMA);
        expect(r[variant].disabledOpacity).toBe(1);
      }
      // `bg-secondary` sets `background-color`; the gradient is a
      // `background-image` and would paint straight over it.
      expect(r.ai.gradientGone, "the ai variant's gradient is removed, not covered").toBe(true);

      // ── link ───────────────────────────────────────────────────────────────
      // Its enabled ink is `--primary`, mid-luminance in both themes, so a muted
      // grey lands 1.11:1 from it in light — a hue difference at equal
      // lightness, i.e. WCAG 1.4.1. It degrades to plain text instead.
      expect(r.link.now.ink, "a disabled link differs from an enabled one in LIGHTNESS, not only hue").toBeGreaterThan(2.5);
      expect(r.link.disabledOpacity, "and not by fading, which moves it towards the teal in dark").toBe(1);
      expect(r.link.hasInsetRing).toBe(false);
      expect(r.link.fillVsSurface, "a disabled link gains no box").toBe(1);
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
      for (const variant of ["default", "outline"]) {
        const b = document.createElement("button");
        b.className = cn(buttonVariants({ variant }));
        b.id = `probe-hover-${variant}`;
        b.textContent = "Save";
        b.disabled = true;
        b.title = "Save your changes first";
        host.appendChild(b);
      }
      document.body.appendChild(host);
    });

    for (const variant of ["default", "outline"] as const) {
      const probe = page.locator(`#probe-hover-${variant}`);
      const atRest = await probe.evaluate((el) => getComputedStyle(el).backgroundColor);

      // A real hover. It is the assertion as much as the setup:
      // `pointer-events-none` was removed from this component on purpose so a
      // disabled control can still be pointed at for its `title`, and
      // reintroducing it would make this line time out with "intercepts pointer
      // events" rather than fail an expectation.
      await probe.hover();

      const hovered = await probe.evaluate((el) => ({
        matchesHover: el.matches(":hover"),
        background: getComputedStyle(el).backgroundColor,
        cursor: getComputedStyle(el).cursor,
        deepest: (
          document.querySelectorAll(":hover")[document.querySelectorAll(":hover").length - 1] as HTMLElement
        )?.id,
      }));

      expect(hovered.deepest, `the disabled ${variant} button is the element under the pointer`).toBe(
        `probe-hover-${variant}`,
      );
      expect(hovered.matchesHover).toBe(true);
      expect(hovered.cursor).toBe("not-allowed");
      // Hit-testing decides `:hover`, so the variant's `hover:bg-*` matches a
      // disabled button. On `default` the disabled fill already outranks it on
      // emission order; on `outline`, which keeps its own fill, the
      // `disabled:hover:*` twins are what hold it — and they are an improvement
      // on `opacity-50`, which took the 3% hover step.
      expect(hovered.background, `hovering a disabled ${variant} does not repaint it`).toBe(atRest);
    }
  });
});
