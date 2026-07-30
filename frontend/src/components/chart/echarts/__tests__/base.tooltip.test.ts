import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { describe, it, expect } from "vitest";
import {
  axisTooltip,
  itemTooltip,
  TOOLTIP_BODY_FONT_SIZE,
  TOOLTIP_HEADER_FONT_SIZE,
  TOOLTIP_INK_WEIGHT,
  TOOLTIP_WEIGHT,
  tooltipInk,
  tooltipSurface,
} from "../base";
import { colorContrast, mixColor } from "../palette";
import { buildPieOption } from "../options/pie";
import { buildCartesianOption } from "../options/cartesian";
import { buildBoxplotOption } from "../options/boxplot";

// egret's chart tooltip was ECharts' factory one, measured live against Metabase (its sample
// question 7 against egret's Sunburst Demo, both 1440x920): a white box, a 1px border IN THE
// HOVERED SLICE'S OWN COLOUR, every cell the same 13px grey, and the share in an accent blue.
// These pin the three things that fixed: no border, a three-level emphasis ramp with the value at
// the top of it, and a header that recedes instead of shouting — all on egret's OWN popover
// tokens, whose legibility is checked against BOTH themes at the bottom of this file.

const cols = [{ name: "region", type: "varchar" }, { name: "m", type: "double" }];
const rows = [["EU", 60], ["US", 40]];
const pieOption = () => buildPieOption(cols, rows, { dimensions: ["region"], metric: "m" }) as never as {
  tooltip: { borderWidth: number; backgroundColor: string; formatter: (p: unknown) => string };
};
const pieTip = (path: string[]) =>
  pieOption().tooltip.formatter({ treePathInfo: [{ name: "" }, ...path.map((name) => ({ name }))] });

describe("a chart tooltip has no border", () => {
  // ECharts draws `tooltip.borderWidth: 1` by default, and for `trigger: "item"` it takes the
  // border's COLOUR from whatever is under the pointer. That is not a legend and it is not a
  // state — it is a colour that looks like it means something and means nothing. Metabase's
  // tooltip has no border at all.
  it("states a zero border width, on every chart type's tooltip", () => {
    const boxplotCols = [{ name: "c", type: "varchar" }, { name: "v", type: "double" }];
    const boxplotRows = [["A", 1], ["A", 2], ["A", 3], ["B", 4], ["B", 5], ["B", 6]];
    const tooltips = {
      shared_item: itemTooltip((v) => String(v)),
      shared_axis: axisTooltip(),
      pie: pieOption().tooltip,
      bar: (buildCartesianOption(cols, rows, { kind: "bar", xAxis: "region", yMetrics: ["m"] }) as never as { tooltip: { borderWidth: number } }).tooltip,
      boxplot: (buildBoxplotOption(boxplotCols, boxplotRows, { category: "c", value: "v" }) as never as { tooltip: { borderWidth: number } }).tooltip,
    };
    for (const [kind, t] of Object.entries(tooltips)) {
      expect(`${kind}:${t.borderWidth}`).toBe(`${kind}:0`);
      expect(t).not.toHaveProperty("borderColor");
    }
  });
});

describe("a pie tooltip's rows read at three distinct levels of emphasis", () => {
  // The concrete colours, in the light palette these tests resolve to (no DOM custom properties in
  // jsdom, so `resolveColor` returns COLOR_FALLBACK.popover* — egret's own light-mode tokens).
  // Stated as values, not as "three different strings": the point is a RAMP, and only actual
  // colours can show one.
  it("mixes the popover ink down in two steps", () => {
    const ink = tooltipInk();
    expect(ink.surface).toBe("#f8f8f8");
    expect(ink.value).toBe("rgb(9, 9, 9)");   // the ink itself
    expect(ink.name).toBe("rgb(45, 45, 45)"); // 0.85 of it
    expect(ink.muted).toBe("rgb(83, 83, 83)");// 0.69 of it — Metabase's own dimming
    // ...and they are in that order as SEEN, which is the property the numbers exist to produce.
    const against = (c: string) => colorContrast(c, ink.surface)!;
    expect(against(ink.value)).toBeGreaterThan(against(ink.name));
    expect(against(ink.name)).toBeGreaterThan(against(ink.muted));
  });

  it("gives the value, the name and the share three different (colour, weight) pairs", () => {
    const html = pieTip(["EU"]);
    // The row under the pointer goes bold throughout, so read an UNfocused one: US.
    const row = /<tr style="font-weight:400">(?:(?!<\/tr>).)*US(?:(?!<\/tr>).)*<\/tr>/.exec(html)![0];
    const cells = [...row.matchAll(/<td style="([^"]*)"[^>]*>(.*?)<\/td>/g)].map((m) => ({
      style: m[1], text: m[2].replace(/<[^>]*>/g, ""),
    }));

    expect(cells.map((c) => c.text)).toEqual(["US", "40", "40%"]);
    // name: the mid ink, and the ROW's regular weight (no weight of its own to override it)
    expect(cells[0].style).toContain("color:rgb(45, 45, 45)");
    expect(cells[0].style).not.toContain("font-weight");
    // value: the full ink, semibold — the number the tooltip exists to report
    expect(cells[1].style).toContain("color:rgb(9, 9, 9)");
    expect(cells[1].style).toContain("font-weight:600");
    // share: semibold too, but the dim ink — Metabase separates it from the value by COLOUR, and
    // egret used to separate it by an accent blue, which said the opposite thing
    expect(cells[2].style).toContain("color:rgb(83, 83, 83)");
    expect(cells[2].style).toContain("font-weight:600");
    expect(html).not.toContain("var(--primary)");
  });

  // Pre-existing behaviour that the ramp must not have inverted: the row the pointer is on goes
  // bold end to end, which means its NAME must outweigh a neighbouring row's value.
  it("still bolds the whole focused row, without turning the ramp upside down", () => {
    const focused = /<tr style="font-weight:700">(?:(?!<\/tr>).)*<\/tr>/.exec(pieTip(["EU"]))![0];
    expect(focused).toContain("EU");
    expect([...focused.matchAll(/font-weight:(\d+)/g)].map((m) => m[1])).toEqual(["700", "700", "700"]);
  });
});

describe("a pie tooltip's header recedes behind the rows", () => {
  // It names the comparison; it is not part of it. It used to be the boldest thing in the tooltip
  // (700, at the row size): the most emphasis on the least information.
  it("is a size smaller than the rows, at regular weight, in the dim ink", () => {
    const header = /<div style="([^"]*)">region<\/div>/.exec(pieTip(["EU"]))![1];
    expect(header).toContain(`font-size:${TOOLTIP_HEADER_FONT_SIZE}px`);
    expect(TOOLTIP_HEADER_FONT_SIZE).toBeLessThan(TOOLTIP_BODY_FONT_SIZE);
    expect(header).toContain("font-weight:400");
    expect(header).toContain(`color:${tooltipInk().muted}`);
    // Lighter than every row cell, not merely lighter than the value.
    const ink = tooltipInk();
    const against = (c: string) => colorContrast(c, ink.surface)!;
    expect(against(ink.muted)).toBeLessThan(against(ink.name));
    expect(TOOLTIP_WEIGHT.header).toBeLessThan(TOOLTIP_WEIGHT.value);
  });
});

// ---------------------------------------------------------------------------------------------
// Both themes.
//
// This is the check that makes the token choice safe rather than merely tasteful. jsdom resolves no
// custom property, so asking `tooltipInk()` would only ever exercise the light-mode fallback — the
// tokens themselves are read straight out of index.css, which is where a regression would land.
// The assertion is the CONTRAST RATIO and not a hex: a hex pins the arithmetic; the ratio pins the
// property the colours were chosen for, and keeps holding if a token is retuned within reason.
describe("the tooltip's surface and ink are legible in both themes", () => {
  // From the project root, not from `import.meta.url`: vitest rewrites that to an http-scheme URL
  // for the module graph, and `readFileSync` cannot open one.
  const css = readFileSync(resolve(process.cwd(), "src/index.css"), "utf8");
  const block = (selector: string) =>
    new RegExp(`^${selector} \\{([\\s\\S]*?)^\\}`, "m").exec(css)![1];
  const token = (selector: string, name: string) =>
    new RegExp(`--${name}:\\s*([^;]+);`).exec(block(selector))![1].trim();

  const themes = {
    light: { surface: token(":root", "popover"), ink: token(":root", "popover-foreground") },
    dark: { surface: token("\\.dark", "popover"), ink: token("\\.dark", "popover-foreground") },
  };

  // WCAG AA for body text. Every level of the ramp has to clear it — a dimmed share is still a
  // number someone has to read.
  const AA = 4.5;

  it.each(Object.entries(themes))("clears WCAG AA at all three ink levels in %s mode", (_theme, t) => {
    for (const weight of Object.values(TOOLTIP_INK_WEIGHT)) {
      const level = mixColor(t.ink, t.surface, weight);
      expect(colorContrast(level, t.surface)).toBeGreaterThanOrEqual(AA);
    }
  });

  // The tokens really are the two this file thinks they are, and really do resolve — a typo'd
  // selector would otherwise make the loop above pass against `undefined`-free garbage.
  it("reads real, parseable colours out of index.css", () => {
    for (const t of Object.values(themes)) {
      expect(colorContrast(t.ink, t.surface)).toBeGreaterThan(1);
      expect(t.surface).toMatch(/^oklch\(/);
    }
    expect(themes.light.surface).not.toBe(themes.dark.surface);
  });

  it("is the popover surface the rest of the app's floating layers use", () => {
    expect(tooltipSurface().backgroundColor).toBe(tooltipInk().surface);
  });
});
