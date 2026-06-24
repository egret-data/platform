import { describe, it, expect } from "vitest";
import { MAPS } from "../registry";

describe("map registry", () => {
  it("has the three maps with normalize behavior", () => {
    expect(Object.keys(MAPS).sort()).toEqual(["us_states", "vietnam", "world"]);
    expect(MAPS.us_states.normalize(" nh ")).toBe("NH");
    expect(MAPS.world.normalize("no")).toBe("NO");
    expect(MAPS.vietnam.normalize(" An Giang ")).toBe("An Giang");
    expect(MAPS.us_states.normalize(null)).toBe("");
  });
});
