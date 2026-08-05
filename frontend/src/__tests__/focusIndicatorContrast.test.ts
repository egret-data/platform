import { readFileSync } from "node:fs";
import path from "node:path";
import { describe, expect, test } from "vitest";

/**
 * THE KEYBOARD FOCUS INDICATOR HAS TO BE VISIBLE, AND THIS PINS THE NUMBER.
 *
 * ── Why 3:1 and not 4.5:1 ────────────────────────────────────────────────────
 * A focus ring is **non-text**. The applicable success criterion is WCAG 2.2
 * **1.4.11 Non-text Contrast** (AA), which asks 3:1 of "visual information
 * required to identify … states" against **adjacent** colours. 4.5:1 is 1.4.3
 * Contrast (Minimum), which governs *text* and does not apply here. Saying which
 * bar is being met matters more than the number: an earlier unit in this
 * programme measured a non-text indicator against the text bar and had to
 * publish a correction, so the bar is named in the assertion itself rather than
 * left as a bare `3`.
 *
 * ── Why this test parses CSS instead of rendering ────────────────────────────
 * jsdom runs no layout and no paint, so a rendered focus ring cannot be measured
 * here at all. But the maths is fully deterministic: `index.css` is the single
 * source of both the indicator colour and every surface it lands on, and
 * OKLCH→sRGB is a closed-form conversion. So the contrast is computed from the
 * stylesheet directly. That also dodges the trap this codebase has hit before —
 * `getComputedStyle` hands back `oklch(…)` for these tokens, and a naive rgb
 * regex reads L, C and H as R, G and B and reports confident garbage. Nothing
 * here regexes a colour into three numbers; the conversion is done properly.
 *
 * These numbers were cross-checked three ways before this test was written:
 * derived numerically (below), read back from a 1×1 canvas in real Chrome, and
 * scanned out of the actual painted pixels of a screenshotted focus ring.
 *
 * ── Why the surface list is long ────────────────────────────────────────────
 * The audit that started this measured `--ring` against `--background` only. But
 * a focus ring lands on whatever surface the control sits on, and the tightest
 * of those is not the page — it is `--secondary`/`--dashboard`, which are a
 * couple of percent darker in light mode and eat ~0.35 of the ratio. Pinning
 * only `--background` would let a value through that fails inside a popover, a
 * toolbar or on the dashboard canvas.
 */

/**
 * Comments are stripped before ANY of this is parsed, and that is not tidiness.
 *
 * The first draft of this test did not strip them, and mutation testing caught it
 * red-handed: three mutations survived — reverting the global fallback to half
 * alpha, nudging it to 90%, and putting Button's halo back to half alpha —
 * because `utilityAlpha()` was matching the utility's name inside the WHY-comment
 * that had just been written above the declaration, and reading an alpha of 1 off
 * the prose while the actual code said 0.5.
 *
 * That is precisely the failure shape this codebase has been bitten by before: a
 * behaviour a comment claims and no assertion pins. Here the comment did not
 * merely fail to pin the behaviour, it actively satisfied the assertion. Strip
 * first, then parse, so these tests can only ever read the code.
 */
const stripCssComments = (source: string) => source.replace(/\/\*[\s\S]*?\*\//g, "");
const stripTsComments = (source: string) =>
  source.replace(/\/\*[\s\S]*?\*\//g, "").replace(/(^|[^:])\/\/.*$/gm, "$1");

const CSS = stripCssComments(readFileSync(path.resolve(__dirname, "../index.css"), "utf8"));
const BUTTON_TSX = stripTsComments(
  readFileSync(path.resolve(__dirname, "../components/ui/button.tsx"), "utf8"),
);

/** WCAG 1.4.11 Non-text Contrast. Named, not inlined, so the bar is greppable. */
const NON_TEXT_CONTRAST_MINIMUM = 3;

/**
 * Every opaque surface a focused control actually sits on. `--muted` is omitted
 * deliberately: in light mode it is `oklch(… / 3%)`, i.e. 3% ink on
 * `--background`, which composites to within a rounding error of `--popover`
 * and is therefore already covered by this list.
 */
const SURFACES = ["background", "card", "popover", "accent", "secondary", "dashboard"] as const;

// ── OKLCH → sRGB (Ottosson), then WCAG relative luminance ────────────────────
// The published oklab→LMS→linear-sRGB matrices. Values are gamut-clipped the way
// a browser clips them when it paints, so the result is the colour that actually
// reaches the screen rather than an out-of-gamut ideal.
function oklchToSrgb(L: number, C: number, hDeg: number): [number, number, number] {
  const h = (hDeg * Math.PI) / 180;
  const a = C * Math.cos(h);
  const b = C * Math.sin(h);
  const l = (L + 0.3963377774 * a + 0.2158037573 * b) ** 3;
  const m = (L - 0.1055613458 * a - 0.0638541728 * b) ** 3;
  const s = (L - 0.0894841775 * a - 1.291485548 * b) ** 3;
  const linear = [
    4.0767416621 * l - 3.3077115913 * m + 0.2309699292 * s,
    -1.2684380046 * l + 2.6097574011 * m - 0.3413193965 * s,
    -0.0041960863 * l - 0.7034186147 * m + 1.707614701 * s,
  ];
  const clamp = (x: number) => Math.min(1, Math.max(0, x));
  const encode = (u: number) => (u <= 0.0031308 ? 12.92 * u : 1.055 * Math.pow(u, 1 / 2.4) - 0.055);
  return linear.map((u) => clamp(encode(clamp(u)))) as [number, number, number];
}

function relativeLuminance([r, g, b]: [number, number, number]): number {
  const lin = (c: number) => (c <= 0.04045 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4));
  return 0.2126 * lin(r) + 0.7152 * lin(g) + 0.0722 * lin(b);
}

function contrastRatio(a: [number, number, number], b: [number, number, number]): number {
  const [x, y] = [relativeLuminance(a), relativeLuminance(b)];
  return (Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05);
}

/** Source-over composite of `fg` at `alpha` onto an opaque `bg`, in gamma space. */
function over(
  fg: [number, number, number],
  alpha: number,
  bg: [number, number, number],
): [number, number, number] {
  return fg.map((c, i) => c * alpha + bg[i] * (1 - alpha)) as [number, number, number];
}

// ── Reading the stylesheet ───────────────────────────────────────────────────
/** The `:root { … }` and `.dark { … }` declaration blocks, by theme. */
function themeBlock(theme: "light" | "dark"): string {
  const selector = theme === "light" ? ":root" : "\\.dark";
  const match = CSS.match(new RegExp(`(?:^|\\n)${selector}\\s*\\{([\\s\\S]*?)\\n\\}`));
  if (!match) throw new Error(`no ${theme} theme block found in index.css`);
  return match[1];
}

/** Resolve one custom property to sRGB. Throws rather than guessing a fallback. */
function token(theme: "light" | "dark", name: string): [number, number, number] {
  const block = themeBlock(theme);
  const decl = block.match(new RegExp(`--${name}:\\s*([^;]+);`));
  if (!decl) throw new Error(`--${name} is not defined in the ${theme} theme`);
  const raw = decl[1].trim();
  const parsed = raw.match(/^oklch\(\s*([\d.]+)\s+([\d.]+)\s+([\d.]+)\s*\)$/);
  if (!parsed) throw new Error(`--${name} in ${theme} is not a plain opaque oklch(): ${raw}`);
  return oklchToSrgb(Number(parsed[1]), Number(parsed[2]), Number(parsed[3]));
}

/**
 * The alpha a Tailwind colour utility applies, from its `/NN` suffix.
 * `outline-ring` with no suffix → 1, with a `/50` suffix → 0.5. Returning a NUMBER rather than
 * asserting on the string is deliberate: it means these tests fail on the
 * *contrast*, so nudging `/50` to `/60` cannot quietly satisfy them.
 */
function utilityAlpha(source: string, utility: string): number {
  const found = source.match(new RegExp(`${utility}(?:/(\\d+))?(?![\\w/-])`));
  if (!found) throw new Error(`utility \`${utility}\` not found — has it been renamed?`);
  return found[1] ? Number(found[1]) / 100 : 1;
}

describe("keyboard focus indicator meets WCAG 1.4.11 non-text contrast (3:1)", () => {
  for (const theme of ["light", "dark"] as const) {
    describe(theme, () => {
      for (const surface of SURFACES) {
        test(`solid --ring reads on --${surface}`, () => {
          const ratio = contrastRatio(token(theme, "ring"), token(theme, surface));
          expect(
            Number(ratio.toFixed(2)),
            `--ring on --${surface} in ${theme}: ${ratio.toFixed(2)}:1`,
          ).toBeGreaterThanOrEqual(NON_TEXT_CONTRAST_MINIMUM);
        });
      }

      /**
       * The global `@layer base { * { … outline-ring … } }` fallback is the ONLY
       * indicator on every focusable element that does not style its own focus —
       * which, measured live, is most of the control-plane navigation. There is
       * no solid companion underneath it to carry the contrast, so whatever alpha
       * it carries has to clear the bar by itself.
       */
      test("the global outline fallback carries the bar on its own", () => {
        const alpha = utilityAlpha(CSS, "outline-ring");
        for (const surface of SURFACES) {
          const bg = token(theme, surface);
          const ratio = contrastRatio(over(token(theme, "ring"), alpha, bg), bg);
          expect(
            Number(ratio.toFixed(2)),
            `global outline-ring @ alpha ${alpha} on --${surface} in ${theme}: ${ratio.toFixed(2)}:1`,
          ).toBeGreaterThanOrEqual(NON_TEXT_CONTRAST_MINIMUM);
        }
      });

      /**
       * `Button`'s focus style is `focus-visible:border-ring` + a `ring-[3px]`
       * halo. On `Input`, `Badge` and `SelectTrigger` the `border-ring` half is
       * load-bearing and the halo is decoration — but those components carry a
       * border-width at rest. Button's default, ghost and link variants do NOT:
       * measured in Chrome, `border-top-width` is `0px`, so `border-ring` sets a
       * colour on a border that is never painted and the halo is the whole
       * indicator. It therefore has to clear the bar unaided, exactly like the
       * global fallback above.
       */
      test("Button's focus halo carries the bar on its own (border-ring paints nothing at 0px border)", () => {
        const alpha = utilityAlpha(BUTTON_TSX, "focus-visible:ring-ring");
        for (const surface of SURFACES) {
          const bg = token(theme, surface);
          const ratio = contrastRatio(over(token(theme, "ring"), alpha, bg), bg);
          expect(
            Number(ratio.toFixed(2)),
            `Button ring @ alpha ${alpha} on --${surface} in ${theme}: ${ratio.toFixed(2)}:1`,
          ).toBeGreaterThanOrEqual(NON_TEXT_CONTRAST_MINIMUM);
        }
      });
    });
  }

  /**
   * `--ring` is a neutral. The fix for the contrast failure was to darken it, and
   * darkening is all it was: a hue or chroma shift would be a visual change
   * nobody asked for and would show up as a tinted focus ring across the whole
   * product. This pins the neutrality so a future "let's make focus blue" lands
   * as a deliberate, reviewed change rather than riding in on a contrast tweak.
   */
  test("--ring stays a neutral in both themes", () => {
    for (const theme of ["light", "dark"] as const) {
      const raw = themeBlock(theme).match(/--ring:\s*([^;]+);/)![1].trim();
      const [, , chroma, hue] = raw.match(/^oklch\(\s*([\d.]+)\s+([\d.]+)\s+([\d.]+)\s*\)$/)!;
      expect(Number(chroma), `${theme} --ring chroma ${chroma}`).toBeLessThanOrEqual(0.02);
      expect(Number(hue), `${theme} --ring hue ${hue}`).toBeGreaterThan(280);
      expect(Number(hue), `${theme} --ring hue ${hue}`).toBeLessThan(290);
    }
  });
});
