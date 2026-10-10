import { existsSync, readFileSync, readdirSync } from "node:fs";
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
const src = (relative: string) =>
  stripTsComments(readFileSync(path.resolve(__dirname, "..", relative), "utf8"));
const BUTTON_TSX = src("components/ui/button.tsx");

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

// ─────────────────────────────────────────────────────────────────────────────
// A menu item is a DIFFERENT defect from the one above, and this is where it is
// pinned.
//
// Everything up to here is about `--ring` being dark enough. A dropdown item, a
// select option and the command palette's rows had no ring to darken: they carry
// `outline-none` / `outline-hidden` and announce focus by swapping their
// background to `--accent` and nothing else. Measured from the tokens in this
// same file, that swap is **1.03:1 in light** (`--accent` oklch(0.97 0 0) against
// a `--popover` of oklch(0.98 0 264.7)) and **1.18:1 in dark**. The bar is 3:1 —
// WCAG 2.2 1.4.11 Non-text Contrast, the same non-text bar named at the top of
// this file, not the 4.5:1 text bar.
//
// ── Why the fix is a ring and not a darker `--accent` ────────────────────────
// Both were on the table. Darkening the token so the swap carries the contrast
// by itself was rejected on measured blast radius, not on taste:
//
//   * `--accent` is read by 63 files and ~110 utility occurrences, of which 55
//     are `hover:bg-accent` and 16 more are `hover:bg-accent/40|/50|/60`. It is
//     the product's RESTING HOVER background — sidebar rows, table rows, filter
//     pills, list rows, toolbar buttons — and only 5 occurrences are the `focus:`
//     variant this defect is about.
//   * to reach 3:1 against `--popover` the light token would have to fall from
//     L 0.97 to **L 0.654**, and the dark token rise from L 0.27 to **L 0.504**.
//     That is a mid-grey. Every mouse-hover in the product would become a slab,
//     and `--accent-foreground` (light: oklch(0.21 …)) would then be sitting on a
//     mid-grey at roughly 4:1 — under the 4.5:1 bar that DOES govern text.
//
// So a hover colour cannot double as a focus indicator without ceasing to be a
// hover colour. The indicator gets its own channel, which is also the shape
// Metabase uses: read out of its shipped stylesheet, its focusable controls are
// `:focus-visible { outline: 2px solid var(--mb-color-focus) }` — a DEDICATED
// focus token, 2px, on `:focus-visible`, with `outline-offset: -2px` (and, in one
// case, `box-shadow: inset 0 0 0 2px`) where the element must not paint outside
// itself. Metabase does not darken its hover background to carry contrast either.
// The rule is copied; the value is not — egret's focus colour is `--ring`, which
// the tests above already hold at 3:1 on every surface including `--accent`.
//
// ── Why `ring-inset` and not an outset ring ─────────────────────────────────
// The list container clips. `DropdownMenuContent`/`SubContent` are
// `overflow-hidden` with only `p-1` (4px) of padding, `SelectContent` is
// `overflow-x-hidden overflow-y-auto`, and the palette's list is
// `overflow-y-auto` — a scroll container clips at its padding box too. A 2px
// outset ring on the first or last item would be cropped along the edge that
// most needs to read. Inset draws inside the item's own box, so the ring's
// neighbours are `--accent` (the focused item's own background) on one side and
// `--popover` on the other, both of which are pinned below.
//
// ── What these tests do NOT pin, and why ────────────────────────────────────
// The variant. `focus-visible:` was chosen expecting it to keep the ring off the
// mouse path, since Radix moves real DOM focus onto an item on pointer move as
// well as on arrow-key navigation. Measured in Chrome against the built bundle,
// THAT EXPECTATION WAS WRONG: a real mousemove over an item leaves it matching
// `:focus-visible` and the ring paints, because Radix focuses from script and
// Chrome treats a script `focus()` the pointer did not target as keyboard-ish.
// (The control: a nav link focused by a real mouse CLICK matches `:focus` and not
// `:focus-visible`, so the discriminator works — it just cannot see through
// Radix.) `dropdown-menu.tsx` carries the full note and why the variant is kept
// anyway. It is unpinned here because jsdom runs no layout and no paint and
// cannot evaluate `:focus-visible` at all, so an assertion about it would be an
// assertion about jsdom. The assertions below therefore pin the utilities and the
// contrast — the parts that ARE deterministic from source — and nothing about
// which device makes the ring appear.
// ─────────────────────────────────────────────────────────────────────────────

/** Every double-quoted string literal in a (comment-stripped) source file. */
const classStrings = (source: string): string[] =>
  [...source.matchAll(/"([^"\n]*)"/g)].map((m) => m[1]);

/**
 * The pixel width of the focus ring a class string declares, or 0 for none.
 * Only an EXPLICIT numeric width counts: bare `ring` is 1px in Tailwind v4 and
 * 3px in v3, and this repo contains both generations of shadcn output, so a bare
 * `ring` would be an assertion about a number nobody in this file can see.
 */
function focusRingWidth(classes: string): number {
  const found = classes.match(/(?:^|\s)focus(?:-visible)?:ring-(\d+)(?![\w/-])/);
  return found ? Number(found[1]) : 0;
}

/** The alpha of the focus ring's COLOUR utility, or 0 when it names no colour. */
function focusRingAlpha(classes: string): number {
  const found = classes.match(/(?:^|\s)focus(?:-visible)?:ring-ring(?:\/(\d+))?(?![\w/-])/);
  if (!found) return 0;
  return found[1] ? Number(found[1]) / 100 : 1;
}

/**
 * The surfaces a menu item's own focus ring lands between: its own focused
 * background, and the popover it is drawn on. Both, because an inset ring abuts
 * `--accent` on its inner edge and sits a padding-width from `--popover` on its
 * outer one, and 1.4.11 asks about adjacent colours on both sides.
 */
const MENU_SURFACES = ["accent", "popover"] as const;

function assertRingCarriesTheBar(classes: string, label: string) {
  const width = focusRingWidth(classes);
  expect(width, `${label}: no explicit focus ring width — ${classes.slice(0, 120)}…`).toBeGreaterThanOrEqual(2);

  const alpha = focusRingAlpha(classes);
  expect(alpha, `${label}: focus ring names no \`ring-ring\` colour`).toBeGreaterThan(0);

  expect(
    /(?:^|\s)focus(?:-visible)?:ring-inset(?:\s|$)/.test(classes),
    `${label}: ring is not inset, and every one of these list containers clips its overflow`,
  ).toBe(true);

  for (const theme of ["light", "dark"] as const) {
    for (const surface of MENU_SURFACES) {
      const bg = token(theme, surface);
      const ratio = contrastRatio(over(token(theme, "ring"), alpha, bg), bg);
      expect(
        Number(ratio.toFixed(2)),
        `${label}: ring @ alpha ${alpha} on --${surface} in ${theme}: ${ratio.toFixed(2)}:1`,
      ).toBeGreaterThanOrEqual(NON_TEXT_CONTRAST_MINIMUM);
    }
  }
}

describe("a menu item that opts out of the outline draws its own indicator", () => {
  /**
   * Deliberately a DIRECTORY sweep rather than a list of the two files that have
   * the defect today. `components/ui` is where shadcn primitives get pasted in,
   * and the shape this catches — "opt out of the outline, then signal focus with
   * a background swap and nothing else" — is shadcn's default output. A
   * `command.tsx`, `context-menu.tsx` or `menubar.tsx` added tomorrow arrives
   * carrying it. (Those three were named as suspects in the plan and none of them
   * exists in this repo; the sweep is how that stays true rather than needing to
   * be re-checked by hand.)
   */
  const uiDir = path.resolve(__dirname, "../components/ui");
  const uiFiles = readdirSync(uiDir).filter((f) => f.endsWith(".tsx"));

  const suspects = uiFiles.flatMap((file) =>
    classStrings(src(`components/ui/${file}`))
      .filter(
        (classes) =>
          /(?:^|\s)outline-(?:none|hidden)(?:\s|$)/.test(classes) &&
          /(?:^|\s)focus:bg-accent(?![\w/-])/.test(classes),
      )
      .map((classes, i) => ({ file, classes, i })),
  );

  test("the sweep finds the elements it is supposed to be guarding", () => {
    // Without this, deleting `focus:bg-accent` everywhere would empty `suspects`
    // and the whole describe below would pass by testing nothing — the exact
    // "empty run scores as a pass" failure this repo's mutation harness guards
    // against. 5 = 4 in dropdown-menu.tsx (sub-trigger, item, checkbox item,
    // radio item) + 1 in select.tsx.
    expect(suspects.map((s) => s.file).sort()).toEqual([
      "dropdown-menu.tsx",
      "dropdown-menu.tsx",
      "dropdown-menu.tsx",
      "dropdown-menu.tsx",
      "select.tsx",
    ]);
  });

  for (const { file, classes, i } of suspects) {
    test(`${file} #${i} carries a solid inset --ring focus indicator`, () => {
      assertRingCarriesTheBar(classes, `${file} #${i}`);
    });
  }

  /**
   * The command palette is the same defect with a different mechanism, and it
   * cannot be swept up by the rule above. DOM focus never leaves its search
   * input — the highlighted row is a VIRTUAL focus driven by a `selected` index —
   * so `:focus-visible` can never match a row and the indicator has to hang off
   * the same conditional that paints `bg-accent`. Which also means it shows for a
   * mouse hover, because `onMouseEnter` sets the same index; that is right here,
   * where "selected" is the only pointer of any kind the list has.
   */
  test("the command palette's virtually-focused row carries the same indicator", () => {
    const active = classStrings(src("components/sqllab/CommandPalette.tsx")).find((c) =>
      /^bg-accent(?![\w/-])/.test(c),
    );
    expect(active, "the palette's active-row class string has moved or been renamed").toBeDefined();

    const width = active!.match(/(?:^|\s)ring-(\d+)(?![\w/-])/);
    expect(Number(width?.[1] ?? 0), `palette active row: ${active}`).toBeGreaterThanOrEqual(2);
    expect(/(?:^|\s)ring-inset(?:\s|$)/.test(active!), `palette active row: ${active}`).toBe(true);

    const alphaMatch = active!.match(/(?:^|\s)ring-ring(?:\/(\d+))?(?![\w/-])/);
    expect(alphaMatch, `palette active row names no ring colour: ${active}`).not.toBeNull();
    const alpha = alphaMatch![1] ? Number(alphaMatch![1]) / 100 : 1;

    for (const theme of ["light", "dark"] as const) {
      for (const surface of MENU_SURFACES) {
        const bg = token(theme, surface);
        const ratio = contrastRatio(over(token(theme, "ring"), alpha, bg), bg);
        expect(
          Number(ratio.toFixed(2)),
          `palette ring @ alpha ${alpha} on --${surface} in ${theme}: ${ratio.toFixed(2)}:1`,
        ).toBeGreaterThanOrEqual(NON_TEXT_CONTRAST_MINIMUM);
      }
    }
  });

  /**
   * `ring-inset` above is justified by a claim about somebody ELSE's class
   * string — that these lists clip, so an outset ring would be cropped. A
   * behaviour a comment claims and no assertion pins is the shape four surviving
   * mutants in this repo have had, so the claim is pinned rather than asserted in
   * prose: if a future edit makes these containers non-clipping, this fails and
   * the `ring-inset` requirement can be revisited on purpose.
   */
  test("the containers these items live in really do clip", () => {
    /**
     * EVERY matching container, not the first one. The draft of this took
     * `.find()`, and mutation testing killed it: `dropdown-menu.tsx` has TWO
     * popover surfaces — `DropdownMenuContent` and `DropdownMenuSubContent` — and
     * making the first one `overflow-visible` left the assertion happily reading
     * the second. A submenu is exactly where an unclipped ring would have gone
     * unnoticed longest.
     */
    const clippers = (source: string, needle: string, expected: number) => {
      const containers = classStrings(src(source)).filter((c) => c.includes(needle));
      expect(
        containers.length,
        `${source}: expected ${expected} class string(s) containing \`${needle}\`, found ${containers.length}`,
      ).toBe(expected);
      // `auto`/`scroll` clip as surely as `hidden` does — a scroll container's
      // padding box is still a clip boundary, so an outset ring on the first or
      // last item is cropped either way.
      return containers.map((c) =>
        /(?:^|\s)overflow(?:-[xy])?-(?:hidden|auto|scroll)(?:\s|$)/.test(c),
      );
    };
    expect(
      clippers("components/ui/dropdown-menu.tsx", "bg-popover", 2),
      "DropdownMenuSubContent and DropdownMenuContent",
    ).toEqual([true, true]);
    expect(clippers("components/ui/select.tsx", "bg-popover", 1), "SelectContent").toEqual([true]);
    expect(clippers("components/sqllab/CommandPalette.tsx", "max-h-80", 1), "palette list").toEqual([true]);
  });
});

/**
 * A RING OFFSET WITHOUT A RING-OFFSET COLOUR IS A WHITE BAND.
 *
 * `ring-offset-<n>` widens the gap between the element and its ring and fills it
 * with `--tw-ring-offset-color`, which defaults to `#fff`. On a dark surface that
 * paints a 2px white hairline *inside* the focus ring — brighter than either
 * neighbour, and reading as a rendering fault rather than as focus.
 *
 * This is a repo-wide sweep rather than the one-line fix it was reported as. The
 * plan handed this over as "real but a single site, not a pattern"; re-reading
 * the code found FOUR, two of which are not focus rings at all but the selected
 * state of a colour swatch, where a white band around a colour is worse still.
 * A sweep is also the only version of this that stays fixed, since the mistake is
 * simply forgetting the second utility.
 *
 * jsdom paints nothing, so the *appearance* of the band cannot be tested here.
 * What can be tested — and is the whole of the defect — is that the offset is
 * always accompanied by a colour for it.
 */
describe("every ring offset names the colour it fills itself with", () => {
  const roots = ["components", "routes", "lib", "hooks"];
  const walk = (dir: string): string[] =>
    readdirSync(dir, { withFileTypes: true }).flatMap((e) => {
      const full = path.join(dir, e.name);
      if (e.isDirectory()) return e.name === "__tests__" ? [] : walk(full);
      return e.name.endsWith(".tsx") || e.name.endsWith(".ts") ? [full] : [];
    });

  const srcRoot = path.resolve(__dirname, "..");
  const files = roots
    .map((r) => path.join(srcRoot, r))
    .filter((d) => existsSync(d))
    .flatMap(walk);

  test("the sweep is actually reading files", () => {
    // Guards the empty-run-scores-as-a-pass failure: a broken path would make
    // `offenders` trivially empty and the assertion below vacuous.
    expect(files.length).toBeGreaterThan(200);
  });

  test("no `ring-offset-<n>` is left without a `ring-offset-<colour>`", () => {
    const offenders: string[] = [];
    for (const file of files) {
      const source = stripTsComments(readFileSync(file, "utf8"));
      for (const classes of classStrings(source)) {
        if (!/(?:^|\s)(?:[\w[\]&:=.-]+:)?ring-offset-\d+(?![\w/-])/.test(classes)) continue;
        // A named colour is anything after `ring-offset-` that is not a number.
        if (/(?:^|\s)(?:[\w[\]&:=.-]+:)?ring-offset-(?!\d)[\w-]+(?:\/\d+)?(?![\w/-])/.test(classes)) continue;
        offenders.push(`${path.relative(srcRoot, file)} — ${classes.slice(0, 140)}`);
      }
    }
    expect(offenders, `ring offsets with no colour:\n  ${offenders.join("\n  ")}`).toEqual([]);
  });
});

// ─────────────────────────────────────────────────────────────────────────────
// ONE FOCUS INDICATOR, NOT FIVE.
//
// #415 and #416 each deferred this on the same reasoning: unifying the focus
// styles is real work with its own review, and conflating it with a contrast fix
// would make both harder to judge. This is that separate change, and the first
// thing it did was correct its own brief.
//
// The brief described TWO generations of shadcn output — an older
// `ring-2 + ring-offset-2` and a newer `border-ring + ring-ring/50 + ring-[3px]`.
// Swept properly (class strings, template literals included, comments stripped)
// the repo had FIVE shapes, and the two named were the two SMALLEST:
//
//   offset ring   (`focus-visible:ring-2 … ring-offset-2`)          7 focus sites
//   border+halo   (`border-ring` + `ring-ring/50` + `ring-[3px]`)   4 sites
//   flush 2px     (`focus-visible:ring-2 focus-visible:ring-ring`) 14 sites  ← the real majority
//   flush 1px     (`focus-visible:ring-1 focus-visible:ring-ring`)  9 sites
//   inset 2px     (#416's menu/select/palette items)                9 sites
//
// So the question was never "which of the two shadcn generations wins". The
// majority shape was already flush-2px-solid, and the job was to finish it.
//
// ── The chosen indicator ────────────────────────────────────────────────────
//   `focus-visible:ring-2 focus-visible:ring-ring`
//   (+ `focus-visible:ring-inset` wherever the container clips)
//
// ── Why solid, and never `border-ring` alone ────────────────────────────────
// #415 measured it: `focus-visible:border-ring` sets a border COLOUR and paints
// nothing unless a border WIDTH already exists. Button's `default`, `ghost` and
// `link` variants report `border-top-width: 0px` in Chrome, so on the product's
// most common control the half-alpha halo was the entire indicator — and a 50%
// composite tops out near 1.73:1 however dark `--ring` gets. `border-ring` is
// therefore allowed only as a SECOND cue on a control that has a border at rest,
// never as the only thing painted. `assertOneIndicator` pins exactly that.
//
// ── Why 2px ─────────────────────────────────────────────────────────────────
// Two independent reasons that happen to agree. WCAG 2.2 **2.4.13 Focus
// Appearance** takes a 2px-thick perimeter as its reference indicator, so 1px is
// under the bar on THICKNESS regardless of colour — the 9 `ring-1` sites below
// were failing a criterion no contrast number would ever have caught. And
// Metabase, which this product copies by policy, ships
// `:focus-visible { outline: 2px solid var(--mb-color-focus) }` — read out of its
// stylesheet in #416. Button's 3px (set by #415 when the halo had to carry the
// bar on its own at half alpha) comes down to 2px now that it is solid.
//
// ── Why flush, and NOT `ring-offset-2` ──────────────────────────────────────
// This is the counter-intuitive one, because the offset gap does exist for a
// reason: it separates the ring from the control's own fill so the ring reads
// against a known colour rather than against whatever the control is painted.
// Three things kill it here anyway.
//
//   1. THE GAP IS FILLED WITH THE WRONG COLOUR. `ring-offset-background` — which
//      is what every offset site in this repo uses, correctly, after #415's sweep
//      — hard-codes `--background`. But a focused control almost never sits on
//      `--background`; it sits on `--card`, `--popover`, `--secondary`,
//      `--accent` or `--dashboard`. In dark mode `--background` is oklch(0.14)
//      and `--card`/`--popover` are oklch(0.21), so a focused Switch inside a
//      card drew a 2px near-black band between the control and its ring. In light
//      mode the same band is oklch(1) against oklch(0.98) and is essentially
//      invisible, which is exactly why it survived review for so long — it is a
//      dark-mode-only artifact. `--background is not a stand-in for the surface`
//      below pins the size of that mismatch.
//   2. IT MAKES THE PINNED CONTRAST NUMBERS MEASURE A BOUNDARY THAT IS NOT
//      PAINTED. Everything above this line measures `--ring` against each
//      SURFACE. With an offset, the ring's inner neighbour is `--background`, not
//      the surface — so on a card or a popover those tests were describing an
//      edge that does not exist. Removing the offset is what makes the geometry
//      match the assertions.
//   3. IT NEEDS 4px OF UNCLIPPED SPACE. #416 established that egret's list
//      containers clip; an offset ring is cropped in all of them, and `ring-inset`
//      — which is the answer there — is mutually exclusive with an offset.
//
// The one thing the offset was buying, separation from the control's own fill, is
// not needed for the bar being met: 1.4.11 and 2.4.13 both ask about the change
// between the FOCUSED and UNFOCUSED states of the same pixels, and those pixels
// are outside the border box, so their unfocused colour is the surface. That is
// the ratio already pinned at the top of this file.
//
// ── What is deliberately NOT swept ──────────────────────────────────────────
// `RuleEditor.tsx`'s two `ring-2 ring-primary ring-offset-1 ring-offset-background`
// strings. They are not focus indicators — they mark the SELECTED colour swatch,
// where the ring really does sit against an arbitrary user-chosen colour and the
// offset really is load-bearing. They stay, and they keep the sweep in the
// describe above non-vacuous.
// ─────────────────────────────────────────────────────────────────────────────

/**
 * Class strings including template literals, which `classStrings` above does not
 * read. It does not need to — the menu items it sweeps are all plain strings —
 * but several of the focus rings here live inside `` `… ${cond ? … : ""}` ``.
 */
const allClassStrings = (source: string): string[] => [
  ...[...source.matchAll(/"([^"\n]*)"/g)].map((m) => m[1]),
  ...[...source.matchAll(/`([^`]*)`/g)].map((m) => m[1]),
];

/** Anything that turns a ring or a border on for a focus state. */
const DECLARES_FOCUS_INDICATOR = /(?:^|\s)(?:dark:)?focus(?:-visible|-within)?:(?:ring|border)-/;

/**
 * Every focus-variant ring COLOUR utility in a class string, as
 * `[name, alpha]`. `ring-inset`, `ring-offset-*` and the bare widths are not
 * colours and are filtered out by name.
 */
function focusRingColours(classes: string): { utility: string; name: string; alpha: number }[] {
  const out: { utility: string; name: string; alpha: number }[] = [];
  for (const m of classes.matchAll(
    /(?:^|\s)(?:dark:)?focus(?:-visible|-within)?:ring-([a-z][\w-]*?)(?:\/(\d+))?(?![\w/[-])/g,
  )) {
    const name = m[1];
    if (name === "inset" || name === "offset" || name.startsWith("offset-")) continue;
    out.push({ utility: m[0].trim(), name, alpha: m[2] ? Number(m[2]) / 100 : 1 });
  }
  return out;
}

/** The declared focus ring width in px — `ring-2` or `ring-[3px]` — or 0. */
function focusWidth(classes: string): number {
  const m = classes.match(
    /(?:^|\s)(?:dark:)?focus(?:-visible|-within)?:ring-(?:(\d+)|\[(\d+)px\])(?![\w/-])/,
  );
  return m ? Number(m[1] ?? m[2]) : 0;
}

describe("the focus indicator is one shape across the whole product", () => {
  const walk = (dir: string): string[] =>
    readdirSync(dir, { withFileTypes: true }).flatMap((e) => {
      const full = path.join(dir, e.name);
      if (e.isDirectory()) return e.name === "__tests__" ? [] : walk(full);
      return e.name.endsWith(".tsx") || e.name.endsWith(".ts") ? [full] : [];
    });

  const srcRoot = path.resolve(__dirname, "..");
  const files = ["components", "routes", "lib", "hooks"]
    .map((r) => path.join(srcRoot, r))
    .filter((d) => existsSync(d))
    .flatMap(walk);

  const declarations = files.flatMap((file) =>
    allClassStrings(stripTsComments(readFileSync(file, "utf8")))
      .filter((classes) => DECLARES_FOCUS_INDICATOR.test(classes))
      .map((classes) => ({ where: path.relative(srcRoot, file), classes })),
  );

  /**
   * The empty-run guard. Without it, renaming the utilities would empty
   * `declarations` and every assertion below would pass by testing nothing —
   * which is one of the four mutation-harness defects this programme recorded.
   */
  test("the sweep finds the focus indicators it is guarding", () => {
    expect(files.length, "walked source files").toBeGreaterThan(200);
    expect(declarations.length, "class strings declaring a focus indicator").toBeGreaterThan(25);
  });

  test("no focus ring is drawn with a ring offset", () => {
    const offenders = declarations
      .filter(({ classes }) => /(?:^|\s)(?:[\w[\]&:=.-]+:)?ring-offset-\d/.test(classes))
      .map(({ where, classes }) => `${where} — ${classes.replace(/\s+/g, " ").slice(0, 110)}`);
    expect(
      offenders,
      `these still separate the ring from the control with a band of \`--background\`:\n  ${offenders.join("\n  ")}`,
    ).toEqual([]);
  });

  test("every focus ring is `--ring` at full alpha", () => {
    const offenders: string[] = [];
    for (const { where, classes } of declarations) {
      for (const { utility, name, alpha } of focusRingColours(classes)) {
        if (name === "ring" && alpha === 1) continue;
        offenders.push(`${where} — \`${utility}\` (in: ${classes.replace(/\s+/g, " ").slice(0, 90)})`);
      }
    }
    expect(
      offenders,
      `a focus ring that is not solid \`--ring\` cannot be reasoned about from the numbers pinned above:\n  ${offenders.join("\n  ")}`,
    ).toEqual([]);
  });

  test("every focus ring is at least 2px thick", () => {
    const offenders: string[] = [];
    for (const { where, classes } of declarations) {
      if (!focusRingColours(classes).some((c) => c.name === "ring")) continue;
      const width = focusWidth(classes);
      if (width >= 2) continue;
      offenders.push(
        `${where} — width ${width || "unset"} (in: ${classes.replace(/\s+/g, " ").slice(0, 90)})`,
      );
    }
    expect(
      offenders,
      `WCAG 2.4.13 takes 2px as the reference thickness; these are thinner:\n  ${offenders.join("\n  ")}`,
    ).toEqual([]);
  });

  test("`focus-visible:border-ring` is never the only thing painted", () => {
    const offenders = declarations
      .filter(
        ({ classes }) =>
          /(?:^|\s)(?:dark:)?focus(?:-visible|-within)?:border-ring(?![\w/-])/.test(classes) &&
          !focusRingColours(classes).some((c) => c.name === "ring"),
      )
      .map(({ where, classes }) => `${where} — ${classes.replace(/\s+/g, " ").slice(0, 110)}`);
    expect(
      offenders,
      `\`border-ring\` paints nothing at \`border-width: 0\` — see #415:\n  ${offenders.join("\n  ")}`,
    ).toEqual([]);
  });

  /**
   * `focus:` fires for a mouse click as well as for the keyboard. The whole point
   * of the `focus-visible` variant everywhere else is that a pointer user does
   * not get a ring; a handful of controls opting back into `focus:` is the same
   * inconsistency in a different dimension. `focus-within:` is fine and stays —
   * it is the only variant that works for a composite widget like `TagCombobox`,
   * where the focusable element is a child of the box being ringed.
   *
   * Scoped to the INDICATOR utilities (`focus:ring-*`, `focus:border-ring`) and
   * not to every `focus:border-*`, because the two inline title inputs — the
   * chart studio's and the dashboard's — legitimately use `focus:bg-background`
   * and `focus:border-border` as a click affordance that says "this is editable
   * now". Those are not indicators and were never trying to be; the defect there
   * was that they were ALSO the only thing painted, which the ring now fixes.
   */
  test("focus rings hang off `focus-visible:` (or `focus-within:`), not bare `focus:`", () => {
    const offenders = declarations
      .filter(({ classes }) =>
        /(?:^|\s)(?:dark:)?focus:(?:ring-|border-ring(?![\w/-]))/.test(classes),
      )
      .map(({ where, classes }) => `${where} — ${classes.replace(/\s+/g, " ").slice(0, 110)}`);
    expect(offenders, `bare \`focus:\` rings on the mouse path:\n  ${offenders.join("\n  ")}`).toEqual(
      [],
    );
  });
});

/**
 * The number behind reason 1 above, so that "the offset band is the wrong colour"
 * is an assertion rather than a claim in a comment — the failure shape this repo
 * has been bitten by four times.
 *
 * If a future change reintroduces `ring-offset-background`, this test is where
 * the cost is written down: it fails the moment `--background` stops being
 * interchangeable with the surfaces controls actually sit on, which it never was.
 */
test("`--background` is not a stand-in for the surface a control sits on", () => {
  const mismatches: string[] = [];
  for (const theme of ["light", "dark"] as const) {
    const bg = token(theme, "background");
    for (const surface of SURFACES) {
      if (surface === "background") continue;
      const ratio = contrastRatio(bg, token(theme, surface));
      if (ratio > 1.1) mismatches.push(`${theme} --${surface}: ${ratio.toFixed(2)}:1`);
    }
  }
  expect(
    mismatches.length,
    `a \`ring-offset-background\` gap on these surfaces paints a visibly different colour, not a seam:\n  ${mismatches.join("\n  ")}`,
  ).toBeGreaterThan(0);
});
