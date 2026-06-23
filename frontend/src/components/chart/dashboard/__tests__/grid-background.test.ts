import { describe, it, expect } from "vitest";
import { generateGridBackground } from "../grid-background";

describe("generateGridBackground", () => {
  const bg = generateGridBackground({ cellWidth: 44, rowHeight: 45, cols: 24, marginX: 6, marginY: 6, stroke: "oklch(0.85 0 0)" });
  it("returns a url(data:image/svg+xml,...) value", () => {
    expect(bg.startsWith('url("data:image/svg+xml')).toBe(true);
  });
  it("bakes the concrete stroke color into the SVG (CSS vars don't resolve in data-URLs)", () => {
    // The caller must pass a resolved color, not var(--border); the helper interpolates it verbatim.
    expect(decodeURIComponent(bg)).toContain("stroke='oklch(0.85 0 0)'");
    expect(bg).not.toContain("var(--");
    expect(bg).not.toContain("hsl(");
  });
  it("draws one rect per column with the cell dimensions", () => {
    const decoded = decodeURIComponent(bg);
    expect((decoded.match(/<rect /g) || []).length).toBe(24);
    expect(decoded).toContain("width='44'");
    expect(decoded).toContain("height='45'");
  });
});
