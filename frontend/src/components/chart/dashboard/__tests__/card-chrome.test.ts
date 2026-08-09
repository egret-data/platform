import { describe, it, expect } from "vitest";
import { cardChromeClass, showsCardBackground } from "../card-chrome";

const card = (card_type: string, options: Record<string, unknown> = {}) => ({ card_type, options });

describe("showsCardBackground", () => {
  it("defaults on for a chart, text or link card", () => {
    expect(showsCardBackground(card("chart"))).toBe(true);
    expect(showsCardBackground(card("text"))).toBe(true);
    expect(showsCardBackground(card("link"))).toBe(true);
  });

  it("defaults OFF for a heading, which egret has always rendered chrome-less", () => {
    expect(showsCardBackground(card("heading"))).toBe(false);
  });

  it("defaults off for a placeholder, which draws its own dashed target", () => {
    expect(showsCardBackground(card("placeholder"))).toBe(false);
  });

  it("lets the stored option win over the default, in BOTH directions", () => {
    // Both directions, because "generalise the heading's chrome-less rendering to any card" also
    // means a heading whose author switched the background on gets chrome. A one-way override
    // would make the toggle a no-op on the one card type that motivated it.
    expect(showsCardBackground(card("chart", { background: false }))).toBe(false);
    expect(showsCardBackground(card("heading", { background: true }))).toBe(true);
  });

  it("ignores a non-boolean stored value rather than coercing it", () => {
    expect(showsCardBackground(card("chart", { background: "false" }))).toBe(true);
    expect(showsCardBackground(card("heading", { background: 1 }))).toBe(false);
  });
});

describe("cardChromeClass", () => {
  it("paints border + card background when the background is on", () => {
    expect(cardChromeClass(card("chart"))).toBe("rounded-sm border bg-card");
  });

  it("falls back to the canvas colour, not to nothing, for a chrome-less card", () => {
    expect(cardChromeClass(card("heading"))).toBe("bg-dashboard");
    expect(cardChromeClass(card("chart", { background: false }))).toBe("bg-dashboard");
  });

  it("gives a placeholder no classes at all", () => {
    expect(cardChromeClass(card("placeholder"))).toBe("");
  });
});
