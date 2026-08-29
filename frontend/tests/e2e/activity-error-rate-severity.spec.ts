import { test, expect } from "@playwright/test";

/**
 * What Activity's error-rate severity actually PAINTS, measured in a real
 * browser because there is nowhere else it can be measured.
 *
 * # The defect
 *
 * `Per endpoint` named the failing endpoints — `Spend pacing 20.0%`,
 * `Feature flag exposure 18.8%` — and rendered them in exactly the same grey as
 * the healthy `0.0%` rows beside them. That is not the "colour alone"
 * anti-pattern, which at least encodes the severity somewhere. It was nothing
 * at all: the column carried a number and no reading of it.
 *
 * # Why a real browser
 *
 * jsdom computes no layout, paints nothing, and does not resolve `oklch()` — it
 * hands the token string back unevaluated (`docs/en/ui-conventions.md` §10). The
 * unit tests in `Activity.test.tsx` therefore assert the two channels jsdom CAN
 * see: the icon (form) and the `sr-only` word. They also assert the class names,
 * and a class assertion would return the same answer with `--destructive` and
 * `--success` swapped, or with either token deleted. Everything below is
 * resolved by Chrome and read back as sRGB pixels: each computed colour is
 * painted over the surface it really sits on, which also composites the alpha
 * in `border-highlight/50` exactly as the compositor does.
 *
 * # Which bars, and why those
 *
 * 1. LEGIBILITY. The rate is small text that carries meaning, so WCAG 1.4.3
 *    applies at 4.5:1 against the surface behind it — including behind a pill,
 *    where the ink sits on a tinted fill and not on the card.
 *
 * 2. DISTINGUISHABILITY, which luminance cannot see. Two tiers can hit 4.5:1
 *    each and still be the same colour as one another. `none` and `failing` are
 *    separated in OKLab hue, not in lightness, because lightness is the axis a
 *    greyscale print or a monochrome terminal already destroys.
 *
 *    `elevated` is deliberately not in that comparison: it is body ink, marked
 *    by FORM. The first draft of this treatment inked it `text-highlight`, and
 *    this spec is what rejected it — amber on the card measures 2.56:1 in
 *    Chrome, a WCAG 1.4.3 failure that every jsdom class assertion had passed
 *    and that `MembersPanel.tsx:206` still ships. `--highlight` is a fill token
 *    (`index.css` calls it a badge/marker colour, with `--highlight-foreground`
 *    as the ink to put ON it), so the middle tier wears it on its border and
 *    keeps the number in body ink.
 *
 *    The second draft died here too, and took the idiom this repo uses
 *    everywhere with it: `bg-destructive/15 text-destructive` measures 3.68:1
 *    in light, because the wash lifts the background out from under an ink that
 *    clears 4.81:1 on the bare card. `bg-destructive text-destructive-foreground`
 *    — the shipped `Button variant="destructive"` pairing — inverts it and
 *    measures 2.83:1 in DARK. Only ink on the unwashed card clears 4.5:1 in
 *    both, so neither marked tier is filled and the ordering is carried by
 *    border strength, by ink, and by two different icons (triangle, octagon).
 *
 * 3. `0.0%` MUST NOT READ AS ORDINARY TEXT. "Zero is good, not missing" is the
 *    judgement this treatment exists to make; if `--success` resolves to
 *    something within noise of `--foreground`/`--muted-foreground`, the page is
 *    back to the defect with extra machinery. Asserted as a hue separation from
 *    both body inks.
 *
 * 4. `--accent` IS RESERVED for hover/focus. Measured rather than grepped: no
 *    tier's ink may resolve to the accent colour, in either theme.
 *
 * Both themes, because `--highlight` and `--success` are redefined under `.dark`
 * (`index.css`) and a treatment that passes in light can fail in dark — the
 * defect this repo already shipped once, in `disabled-button-paint.spec.ts`.
 *
 * # Prerequisites
 *
 * The Vite dev server only (`npm run dev`) — no backend, no login, no
 * workspace. The page hosts the compiled stylesheet; the probes are built from
 * the real module, so `ERROR_RATE_INK` and `ERROR_RATE_PILL` are the strings
 * the product ships.
 *
 *   EGRET_E2E_ERROR_RATE_PAINT=1 npx playwright test activity-error-rate-severity
 */
const ENABLED = process.env.EGRET_E2E_ERROR_RATE_PAINT === "1";

/** WCAG 1.4.3 Contrast (Minimum), normal text. */
const TEXT_CONTRAST = 4.5;

/**
 * Minimum OKLab hue separation, in degrees, for two inks to be different
 * COLOURS rather than two lightnesses of one. 30° is a conservative floor: the
 * three tokens are nominally ~155° (green), ~65-70° (amber) and ~22-28° (red)
 * apart in the source, so a treatment that only clears 30° has drifted a long
 * way from what was designed.
 */
const HUE_SEPARATION = 30;

/** Below this OKLab chroma an ink is grey, and a hue angle over it is noise. */
const NEUTRAL_CHROMA = 0.02;

const TIERS = ["none", "elevated", "failing"] as const;
type Tier = (typeof TIERS)[number];

type Reading = {
  /** Ink against whatever is directly behind it — the pill fill where there is
   * one, the card where there is not. */
  inkContrast: number;
  /** Pill fill against the card, for the tiers that have one. */
  fillContrast: number;
  borderWidth: number;
  borderContrast: number;
  hue: number;
  chroma: number;
  ink: string;
  fill: string;
};

type Measurement = {
  tiers: Record<Tier, Reading>;
  /** The two body inks a reading must not be mistaken for. */
  foreground: { hue: number; chroma: number; ink: string };
  mutedForeground: { hue: number; chroma: number; ink: string };
  /** The reserved hover/focus token. */
  accent: { hue: number; chroma: number; ink: string };
};

async function measure(page: import("@playwright/test").Page, theme: "light" | "dark"): Promise<Measurement> {
  return page.evaluate(async (mode) => {
    const mod = await import("/src/routes/explore/dataapi/Activity.tsx");
    const utils = await import("/src/lib/utils.ts");
    const { ERROR_RATE_INK, ERROR_RATE_PILL } = mod as {
      ERROR_RATE_INK: Record<string, string>;
      ERROR_RATE_PILL: Record<string, string>;
    };
    const { cn } = utils as { cn: (...c: string[]) => string };

    // The theme lives on <html> and the `dark` variant is `&:is(.dark *)`, so a
    // "light" probe nested inside a dark document would silently be a dark one.
    document.documentElement.classList.toggle("dark", mode === "dark");

    const host = document.createElement("div");
    host.style.cssText = "position:fixed;left:0;top:0;z-index:99999";
    // `bg-card` is what the per-endpoint table sits on — `Table` inside a
    // `rounded-md border`. Measuring against `bg-background` would be a
    // measurement of a surface these readings never touch.
    const pane = document.createElement("div");
    pane.className = "bg-card p-4 flex flex-col gap-1";

    const tiers = ["none", "elevated", "failing"];
    for (const tier of tiers) {
      const span = document.createElement("span");
      span.id = `tier-${tier}`;
      // Exactly the composition `ErrorRate` ships for `size="cell"`: the marked
      // tiers get the pill geometry, the clean one does not.
      const marked = tier !== "none";
      span.className = cn(
        "inline-flex items-center gap-1 tabular-nums",
        ERROR_RATE_INK[tier],
        marked && "rounded px-1.5 py-0.5 text-xs font-medium",
        ERROR_RATE_PILL[tier],
      );
      span.textContent = "12.3%";
      pane.appendChild(span);
    }
    // The two inks the "good" reading must not be confused with, plus the
    // reserved accent — same surface, same probe shape.
    for (const [id, cls] of [
      ["body-foreground", "text-foreground"],
      ["body-muted", "text-muted-foreground"],
      ["reserved-accent", "text-accent"],
    ] as const) {
      const span = document.createElement("span");
      span.id = id;
      span.className = cls;
      span.textContent = "12.3%";
      pane.appendChild(span);
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
    const oklab = (c: number[]) => {
      const [r, g, b] = [channel(c[0]), channel(c[1]), channel(c[2])];
      const l = Math.cbrt(0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b);
      const m = Math.cbrt(0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b);
      const s = Math.cbrt(0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b);
      const A = 1.9779984951 * l - 2.428592205 * m + 0.4505937099 * s;
      const B = 0.0259040371 * l + 0.7827717662 * m - 0.808675766 * s;
      return { chroma: Math.hypot(A, B), hue: ((Math.atan2(B, A) * 180) / Math.PI + 360) % 360 };
    };
    const hex = (c: number[]) => "#" + c.map((v) => v.toString(16).padStart(2, "0")).join("");
    const round = (n: number) => Math.round(n * 1000) / 1000;

    const surface = paint(getComputedStyle(pane).backgroundColor, [255, 255, 255]);
    const readInk = (id: string) => {
      const cs = getComputedStyle(document.getElementById(id)!);
      const ink = paint(cs.color, surface);
      const lab = oklab(ink);
      return { hue: Math.round(lab.hue * 10) / 10, chroma: round(lab.chroma), ink: hex(ink) };
    };

    const out: Record<string, unknown> = {};
    const readings: Record<string, unknown> = {};
    for (const tier of tiers) {
      const el = document.getElementById(`tier-${tier}`)!;
      const cs = getComputedStyle(el);
      // The pill fill is semi-transparent (`/15`), so it must be composited
      // onto the card before the ink is measured against it.
      const fill = paint(cs.backgroundColor, surface);
      const ink = paint(cs.color, fill);
      const lab = oklab(ink);
      const border = paint(cs.borderTopColor, surface);
      readings[tier] = {
        inkContrast: ratio(ink, fill),
        fillContrast: ratio(fill, surface),
        borderWidth: parseFloat(cs.borderTopWidth) || 0,
        borderContrast: ratio(border, surface),
        hue: Math.round(lab.hue * 10) / 10,
        chroma: round(lab.chroma),
        ink: hex(ink),
        fill: hex(fill),
      };
    }
    out.tiers = readings;
    out.foreground = readInk("body-foreground");
    out.mutedForeground = readInk("body-muted");
    out.accent = readInk("reserved-accent");
    host.remove();
    return out as unknown as Measurement;
  }, theme);
}

/** Shortest angular distance between two hues, in degrees. */
function hueGap(a: number, b: number): number {
  const d = Math.abs(a - b) % 360;
  return d > 180 ? 360 - d : d;
}

test.describe("An error rate is painted with its severity, in both themes", () => {
  test.skip(!ENABLED, "opt-in: set EGRET_E2E_ERROR_RATE_PAINT=1 with `npm run dev` up");

  for (const theme of ["light", "dark"] as const) {
    test(`${theme} mode: every tier is legible, and no two are the same colour`, async ({ page }) => {
      await page.goto("/login");
      const m = await measure(page, theme);

      // EVERY tier, including the one that is body ink: this is the assertion
      // the first draft failed, and dropping `elevated` from it because it is
      // "only body ink now" would retire the measurement that found the bug.
      for (const tier of TIERS) {
        const r = m.tiers[tier];
        // Legible against what is directly behind it, pill fill included.
        expect(r.inkContrast, `${tier}: ${r.ink} on ${r.fill} must clear WCAG 1.4.3`).toBeGreaterThanOrEqual(
          TEXT_CONTRAST,
        );
      }

      // The two HUED tiers must be colours at all — a hue angle over a grey ink
      // is noise, and these two carry meaning in hue.
      for (const tier of ["none", "failing"] as const) {
        expect(
          m.tiers[tier].chroma,
          `${tier}: ${m.tiers[tier].ink} is neutral grey, which is the defect`,
        ).toBeGreaterThan(NEUTRAL_CHROMA);
      }
      expect(
        hueGap(m.tiers.none.hue, m.tiers.failing.hue),
        `good (${m.tiers.none.ink}) and failing (${m.tiers.failing.ink}) are the same hue`,
      ).toBeGreaterThanOrEqual(HUE_SEPARATION);

      // And `elevated` is separated from both by NOT being a hue: it is the
      // body ink exactly, so a reader cannot mistake its neutrality for a
      // fourth colour that happens to be dull.
      expect(m.tiers.elevated.ink, "elevated should be plain body ink").toBe(m.foreground.ink);
    });

    test(`${theme} mode: 0.0% reads as good rather than as ordinary text`, async ({ page }) => {
      await page.goto("/login");
      const m = await measure(page, theme);
      const good = m.tiers.none;

      // The whole judgement in this treatment: a zero error rate is the best
      // measurement there is, and the column already spends `—` on "no
      // measurement". If `--success` lands within noise of the body ink, the
      // reading is back to being invisible.
      for (const body of [m.foreground, m.mutedForeground] as const) {
        if (body.chroma > NEUTRAL_CHROMA) {
          expect(
            hueGap(good.hue, body.hue),
            `success ${good.ink} is the same hue as body text ${body.ink}`,
          ).toBeGreaterThanOrEqual(HUE_SEPARATION);
        } else {
          // The usual case: body text is neutral, so "is it a colour" is the
          // whole question and chroma answers it.
          expect(
            good.chroma,
            `success ${good.ink} is no more coloured than neutral body text ${body.ink}`,
          ).toBeGreaterThan(body.chroma + NEUTRAL_CHROMA);
        }
      }
    });

    test(`${theme} mode: no tier borrows --accent, which belongs to hover and focus`, async ({ page }) => {
      await page.goto("/login");
      const m = await measure(page, theme);
      for (const tier of TIERS) {
        expect(m.tiers[tier].ink, `${tier} is painted in the reserved accent colour`).not.toBe(
          m.accent.ink,
        );
      }
    });

    test(`${theme} mode: nothing, a soft outline, a strong outline — in that order`, async ({ page }) => {
      await page.goto("/login");
      const m = await measure(page, theme);

      // The form ladder, which is the half that survives greyscale. It is an
      // ORDERING, so "elevated looks more urgent than failing" is a defect even
      // when both are individually visible — asserted as a relation between the
      // two readings rather than as two fixed numbers that can drift apart.
      // These are "perceptible" floors, not WCAG 1.4.11's 3:1: the border is
      // reinforcement, and the icon, the ink and the `sr-only` word each carry
      // the distinction on their own.
      expect(m.tiers.elevated.borderWidth, "elevated should be outlined").toBeGreaterThan(0);
      expect(m.tiers.failing.borderWidth, "failing should be outlined").toBeGreaterThan(0);
      expect(
        m.tiers.failing.borderContrast,
        "the failing outline must be stronger than the elevated one",
      ).toBeGreaterThan(m.tiers.elevated.borderContrast);
      expect(m.tiers.elevated.borderContrast, "the elevated outline is invisible").toBeGreaterThan(1.05);

      // Neither marked tier is filled — see the header. A fill is exactly what
      // broke the contrast, twice, in two different directions.
      for (const tier of ["elevated", "failing"] as const) {
        expect(m.tiers[tier].fillContrast, `${tier} should not be filled`).toBeCloseTo(1, 2);
      }

      // And the clean tier is plain text: no fill, no outline. That absence is
      // what makes the other two read as marked rather than as three variants
      // of one badge.
      expect(m.tiers.none.fillContrast, "the clean tier should carry no fill").toBeCloseTo(1, 2);
      expect(m.tiers.none.borderWidth, "the clean tier should carry no outline").toBe(0);
    });
  }
});
