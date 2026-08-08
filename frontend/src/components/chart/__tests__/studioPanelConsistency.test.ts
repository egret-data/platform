import { describe, it, expect } from "vitest";
import { readFileSync, readdirSync, statSync } from "node:fs";
import { join } from "node:path";

/**
 * SOURCE-LEVEL invariants for the chart studio's config sidebar.
 *
 * WHY A SOURCE HARNESS AND NOT A RENDER TEST. Every rule below is about LAYOUT — how wide a
 * control is, whether two controls line up, whether a label reads as a heading. jsdom runs no
 * layout engine: `getBoundingClientRect()` returns zeroes there, `getComputedStyle` resolves no
 * Tailwind utility, and a rendered `<SelectTrigger className="h-8 text-sm">` is indistinguishable
 * from one carrying `w-full`. A render test therefore cannot fail when the defect returns, which is
 * the only property that makes a test worth having. These read the source instead, which is exactly
 * the shape `focusIndicatorContrast.test.ts` settled on for the same reason.
 *
 * The real widths were measured in the browser on the kind deployment and the numbers are recorded
 * in the PR body — a source assertion pins the RULE, the browser proved the PIXELS.
 */

// `import.meta.url` is not a file: URL under vitest's jsdom environment ("The URL must be of scheme
// file"), so the root comes from the cwd vitest is launched with, which is always `frontend/`.
const SRC = join(process.cwd(), "src");

function walk(dir: string, out: string[] = []): string[] {
  for (const name of readdirSync(dir)) {
    const p = join(dir, name);
    if (statSync(p).isDirectory()) {
      if (name === "__tests__" || name === "node_modules") continue;
      walk(p, out);
    } else if (p.endsWith(".tsx") || p.endsWith(".ts")) {
      out.push(p);
    }
  }
  return out;
}

/**
 * THE CONFIG SIDEBAR, and nothing else that happens to live under `components/chart`.
 *
 * `viz/` declares the Data-tab controls, `settings/` the Display and Conditional-formatting ones,
 * `config/` the field zones, and `ChartExplore.tsx` owns the shell around them. The chart BODIES —
 * `ObjectDetail.tsx`, `ChartTable.tsx`, `BigNumber.tsx` — are excluded on purpose: they draw data,
 * not controls, and `ObjectDetail`'s uppercase `<dt>` is a rendered field name inside the chart, so
 * a rule about the panel's label hierarchy has no jurisdiction over it. Walking the whole directory
 * and then exempting the misses one by one would invert that: the list would be whatever the
 * assertion happened to trip over.
 */
const PANEL_FILES = [
  ...walk(join(SRC, "components", "chart", "viz")),
  ...walk(join(SRC, "components", "chart", "settings")),
  ...walk(join(SRC, "components", "chart", "config")),
  join(SRC, "components", "chart", "SortRows.tsx"),
  join(SRC, "components", "chart", "TemporalUnitSelect.tsx"),
  join(SRC, "components", "chart", "StudioPanel.tsx"),
  join(SRC, "components", "chart", "ChartTypeGallery.tsx"),
  join(SRC, "components", "chart", "VizSwitcher.tsx"),
  join(SRC, "routes", "explore", "ChartExplore.tsx"),
];

const read = (f: string) => readFileSync(f, "utf8");

describe("studio config panel — layout invariants", () => {
  /**
   * EVERY select trigger in the studio states its own width.
   *
   * shadcn v4's `SelectTrigger` defaults to `w-fit`, so a trigger that named no width sized itself
   * to THE TEXT OF ITS CURRENT VALUE. Measured live on the kind deployment in the panel's 255px
   * column: Dataset 143px, X-axis 162px, Series 82px, "X metric (optional)" 211px, Region 186px,
   * "Funnel — one narrowing band" 229px — against a Row limit Input at the full 255px. No two rows
   * shared a right edge, and picking a different value visibly resized the control under the
   * cursor. Two shapes, one rule:
   *
   *   - BLOCK (label above, control beneath, inside `space-y-1`) → `w-full`.
   *   - INLINE (`flex items-center justify-between`, label left, control right) → a fixed `w-*`,
   *     which is what `AutoToggle` (`w-24`) and Missing values (`w-28`) already did. A fixed width
   *     cannot jitter either, which is the property being enforced — not `w-full` specifically.
   *
   * So the assertion is "names a width", not "is full width": it is satisfiable both ways and
   * still fails for every `w-fit` default.
   */
  it("every select trigger in the studio panel states its width", () => {
    const offenders: string[] = [];
    for (const f of PANEL_FILES) {
      const src = read(f);
      src.split("\n").forEach((line, i) => {
        if (!line.includes("SelectTrigger")) return;
        const m = line.match(/className="([^"]*)"/);
        if (!m) return;
        const cls = m[1];
        if (/(^|\s)(w-|flex-1)/.test(cls)) return;
        offenders.push(`${f.slice(SRC.length + 1)}:${i + 1} className="${cls}"`);
      });
    }
    expect(offenders).toEqual([]);
  });

  /**
   * The Data tab had NO hierarchy: every field label ("Dataset", "X-axis", "Metric", "Row limit")
   * used the same uppercase/tracked/muted treatment as a section heading, so a group of controls
   * and a single control shouted identically. Metabase's own setting labels are
   * `<Text component="label" fw="bold">` — sentence case, foreground colour, no letter-spacing
   * (`ChartSettingsWidget.tsx`) — and its only uppercase text in the sidebar is nothing at all.
   *
   * `SectionLabel` keeps the uppercase treatment, because it IS the heading level: it is what makes
   * "Series"/"Format"/"Totals" read as groups above the sentence-case controls inside them.
   */
  it("field labels are not styled as section headings", () => {
    const offenders: string[] = [];
    for (const f of PANEL_FILES) {
      if (f.endsWith("SectionLabel.tsx")) continue;
      const src = read(f);
      src.split("\n").forEach((line, i) => {
        // A label/field class that shouts. Section headings go through SectionLabel, which is
        // exempted above; anything else spelling out the same utilities is a field label.
        if (!/uppercase/.test(line)) return;
        if (!/text-xs|text-\[11px\]/.test(line)) return;
        // `text-transform` inside a comment is not a rendered class. Tailwind v4 scans comments for
        // class candidates when BUILDING, but this harness reads intent, and a WHY-comment naming
        // the utility it removed must not re-fail the test it documents.
        if (/^\s*(\/\/|\*|\/\*)/.test(line)) return;
        offenders.push(`${f.slice(SRC.length + 1)}:${i + 1} ${line.trim().slice(0, 90)}`);
      });
    }
    expect(offenders).toEqual([]);
  });
});
