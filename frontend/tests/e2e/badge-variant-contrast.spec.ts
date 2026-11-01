import { test, expect } from "@playwright/test";

/**
 * WHAT EVERY `Badge` VARIANT PAINTS, IN BOTH THEMES, measured in a real browser.
 *
 * # The defect
 *
 * In the light theme `Badge variant="default"` and `Button variant="default"`
 * paint white `--primary-foreground` on the teal `--primary`. At
 * `oklch(0.59 0.15 185)` that measured 3.50:1, under WCAG 2.2 1.4.3's 4.5:1 for
 * normal text (the badge is 12px medium and the button 13px medium, so the
 * large-text 3:1 bar does not apply). The fix darkens the light `--primary`
 * token; the dark theme already passed and is unchanged. Every other variant
 * passed in both themes before the change and is measured here so it stays so.
 *
 * Because `--primary` is also the light theme's link ink (`text-primary`), its
 * tinted surfaces (`bg-primary/10` behind `text-primary`: avatars, the active
 * settings item, SQL Lab's chip), the hover fill (`bg-primary/90`) and a focus
 * outline (`outline-primary`, non-text, 3:1), those pairs are probed too.
 *
 * # Why a real browser, and why a canvas
 *
 * jsdom composites nothing and does not resolve `oklch()`; the tokens and the
 * alpha washes are only real once Chrome has painted them. Each computed colour
 * is painted over the surface it sits on in a 1×1 canvas and read back as sRGB,
 * the same instrument as `dataapi-badge-ink.spec.ts` (see its header for the
 * traps — refused colours, uncomposited alpha — that the controls below guard).
 *
 * # Why the variants are IMPORTED
 *
 * The class strings come from the shipped `badgeVariants` and `STATUS_TONE`, so
 * a variant rewritten tomorrow changes what this spec paints.
 *
 * # Surfaces
 *
 * A badge sits on the page (`--background`), in a card or a table
 * (`--card`), and on a hovered or muted row (`--muted`). In dark these are
 * three different colours, and a translucent fill composites differently on
 * each; the weakest is what a reader gets.
 *
 * # The outline border
 *
 * Reported, not asserted. The border of an outline badge is the shared
 * `--border` every card and input wears; the word inside the badge is what
 * identifies it, so WCAG 1.4.11 does not require the border to reach 3:1.
 *
 * # Prerequisites
 *
 * The Vite dev server only (`npm run dev`) — no backend, no login.
 *
 *   EGRET_E2E_BADGE_CONTRAST=1 npx playwright test badge-variant-contrast
 */
const ENABLED = process.env.EGRET_E2E_BADGE_CONTRAST === "1";

/** WCAG 2.2 1.4.3 Contrast (Minimum), normal text: the badge is 12px medium. */
const TEXT_CONTRAST = 4.5;

/** Every variant `badgeVariants` declares. A name it does not know paints no fill and fails the control below. */
const VARIANTS = ["default", "secondary", "destructive", "outline", "success", "warning", "processing"] as const;

const SURFACES = ["bg-background", "bg-card", "bg-muted"] as const;

/** WCAG 2.2 1.4.11 Non-text Contrast: a focus outline in `--primary`. */
const NON_TEXT_CONTRAST = 3;

/**
 * Text pairs built on `--primary` outside `Badge`, as call sites write them.
 * `Button`'s own variants are imported below; these are the utility pairs.
 */
const PRIMARY_PAIRS: [string, string][] = [
  ["primary.hover-fill", "bg-primary/90 text-primary-foreground"],
  ["primary.link-ink", "text-primary"],
  ["primary.tint-10", "bg-primary/10 text-primary"],
  ["primary.tint-15-foreground", "bg-primary/15 text-foreground"],
];

type Reading = { name: string; ratio: number; ink: string; fill: string; border: number };

async function measure(
  page: import("@playwright/test").Page,
  theme: "light" | "dark",
  surface: (typeof SURFACES)[number],
): Promise<{
  unknown: string[];
  readings: Reading[];
  outline: Reading;
  controls: Record<string, Reading>;
  composite: number[][];
}> {
  return page.evaluate(
    async ({ mode, surfaceClass, variants, primaryPairs }) => {
      const badge = (await import("/src/components/ui/badge.tsx")) as {
        badgeVariants: (p: { variant: string }) => string;
      };
      const status = (await import("/src/components/ui/status-badge.tsx")) as {
        STATUS_TONE: Record<string, string>;
      };
      const button = (await import("/src/components/ui/button.tsx")) as {
        buttonVariants: (p: { variant: string }) => string;
      };
      const utils = (await import("/src/lib/utils.ts")) as { cn: (...c: string[]) => string };

      document.documentElement.classList.toggle("dark", mode === "dark");

      const host = document.createElement("div");
      host.style.cssText = "position:fixed;left:0;top:0;z-index:99999";
      // `bg-muted` is translucent in light (3% ink): lay it on the page, as a row does.
      const page = document.createElement("div");
      page.className = "bg-background";
      const pane = document.createElement("div");
      pane.className = `${surfaceClass} p-4 flex flex-col gap-1`;
      page.appendChild(pane);
      host.appendChild(page);

      const specs: [string, string][] = [
        ...variants.map((v) => [`variant.${v}`, badge.badgeVariants({ variant: v })] as [string, string]),
        ...Object.entries(status.STATUS_TONE).map(
          ([k, v]) => [`StatusBadge.${k}`, utils.cn(badge.badgeVariants({ variant: "outline" }), v)] as [string, string],
        ),
        ["Button.default", button.buttonVariants({ variant: "default" })],
        ["Button.link", button.buttonVariants({ variant: "link" })],
        ...primaryPairs.map(([id, cls]) => [id, `inline-flex px-2 py-0.5 text-xs font-medium ${cls}`] as [string, string]),
      ];
      const mk = (id: string, cls: string) => {
        const span = document.createElement("span");
        span.id = `probe-${id}`;
        span.className = cls;
        span.textContent = "Allowed 3";
        pane.appendChild(span);
      };
      for (const [id, cls] of specs) mk(id, cls);
      // The focus outline drawn in `--primary`, read as a border of the same colour.
      mk("outline-primary", "inline-flex border-2 border-primary px-2 py-0.5 text-xs");
      mk("control-pass", "inline-flex rounded-md border px-2 py-0.5 text-xs font-medium text-foreground");
      const selfInk = document.createElement("span");
      selfInk.id = "probe-control-fail";
      selfInk.className = "inline-flex rounded-md px-2 py-0.5 text-xs font-medium";
      selfInk.style.color = "transparent";
      selfInk.textContent = "Allowed 3";
      pane.appendChild(selfInk);

      document.body.appendChild(host);
      await new Promise((r) => requestAnimationFrame(() => requestAnimationFrame(r)));

      const canvas = document.createElement("canvas");
      canvas.width = canvas.height = 1;
      const ctx = canvas.getContext("2d", { willReadFrequently: true })!;
      const accepted = (color: string): boolean => {
        ctx.fillStyle = "#000000";
        ctx.fillStyle = color;
        const fromBlack = ctx.fillStyle;
        ctx.fillStyle = "#ffffff";
        ctx.fillStyle = color;
        return ctx.fillStyle === fromBlack;
      };
      const paint = (color: string, backdrop: number[]) => {
        if (!accepted(color)) throw new Error(`canvas refused the colour \`${color}\``);
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
      const luminance = (c: number[]) => 0.2126 * channel(c[0]) + 0.7152 * channel(c[1]) + 0.0722 * channel(c[2]);
      const ratio = (a: number[], b: number[]) => {
        const [x, y] = [luminance(a), luminance(b)];
        return Math.round(((Math.max(x, y) + 0.05) / (Math.min(x, y) + 0.05)) * 100) / 100;
      };
      const hex = (c: number[]) => "#" + c.map((v) => v.toString(16).padStart(2, "0")).join("");

      const pageRgb = paint(getComputedStyle(page).backgroundColor, [255, 255, 255]);
      const surfaceRgb = paint(getComputedStyle(pane).backgroundColor, pageRgb);
      const composite = [surfaceRgb, paint("rgba(0,0,0,0.5)", surfaceRgb), surfaceRgb.map((v) => Math.round(v / 2))];

      const read = (id: string): Reading => {
        const cs = getComputedStyle(document.getElementById(`probe-${id}`)!);
        const fill = paint(cs.backgroundColor, surfaceRgb);
        const ink = paint(cs.color, fill);
        const edge = paint(cs.borderTopColor, surfaceRgb);
        return { name: id, ratio: ratio(ink, fill), ink: hex(ink), fill: hex(fill), border: ratio(edge, surfaceRgb) };
      };
      // A name `badgeVariants` does not know yields the base classes only; every listed variant must add its own.
      const base = badge.badgeVariants({ variant: "__none__" });
      const unknown = variants.filter((v) => badge.badgeVariants({ variant: v }) === base);
      const out = {
        unknown,
        readings: specs.map(([id]) => read(id)),
        outline: read("outline-primary"),
        controls: { pass: read("control-pass"), fail: read("control-fail") },
        composite,
      };
      host.remove();
      document.documentElement.classList.remove("dark");
      return out;
    },
    { mode: theme, surfaceClass: surface, variants: [...VARIANTS], primaryPairs: PRIMARY_PAIRS },
  );
}

test.describe("every Badge variant, the primary Button and the --primary pairs are legible, in both themes, on every surface", () => {
  test.skip(!ENABLED, "opt-in: set EGRET_E2E_BADGE_CONTRAST=1 with `npm run dev` up");

  for (const theme of ["light", "dark"] as const) {
    for (const surface of SURFACES) {
      test(`${theme} on ${surface}: every variant's text clears WCAG 1.4.3`, async ({ page }) => {
        await page.goto("/login");
        const m = await measure(page, theme, surface);
        for (const r of m.readings) {
          console.log(`${theme}\t${surface}\t${r.name}\ttext ${r.ratio}\tborder ${r.border}\t${r.ink} on ${r.fill}`);
        }

        // Controls first: an instrument returning nothing reads like a pass.
        expect(m.controls.pass.ratio, "control: body ink must clear the bar").toBeGreaterThanOrEqual(TEXT_CONTRAST);
        expect(m.controls.fail.ratio, "control: transparent ink must read 1:1").toBeLessThan(1.01);
        const [, got, want] = m.composite;
        for (let i = 0; i < 3; i++) {
          expect(Math.abs(got[i] - want[i]), "control: the probe is not compositing alpha").toBeLessThanOrEqual(2);
        }
        expect(m.readings.length).toBe(VARIANTS.length + 6 + 2 + PRIMARY_PAIRS.length);
        expect(m.unknown, "variants `badgeVariants` does not declare").toEqual([]);

        console.log(`${theme}\t${surface}\toutline-primary\tborder ${m.outline.border}`);
        expect(
          m.outline.border,
          `outline-primary on ${surface}: ${m.outline.border}:1 (WCAG 1.4.11, non-text)`,
        ).toBeGreaterThanOrEqual(NON_TEXT_CONTRAST);

        for (const r of m.readings) {
          expect(r.ratio, `${r.name}: ink ${r.ink} on ${r.fill} measures ${r.ratio}:1`).toBeGreaterThanOrEqual(
            TEXT_CONTRAST,
          );
        }
      });
    }
  }
});
