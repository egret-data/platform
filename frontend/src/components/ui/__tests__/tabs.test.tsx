import { describe, expect, it } from "vitest";

import { tabsListClass } from "@/components/ui/tabs";

describe("underline tab strip", () => {
  // KI-269: the strip scrolls (overflow-x-auto), which clips at its padding box. The active
  // trigger's 2px underline is pulled 1px down (-mb-px) over the strip's border, so the strip
  // needs 1px of bottom padding or the underline is cut.
  it("reserves bottom padding so the active underline is not clipped", () => {
    const cls = tabsListClass("underline").split(/\s+/);
    expect(cls).toContain("overflow-x-auto");
    expect(cls).toContain("pb-px");
  });
});
