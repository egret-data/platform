import { describe, it, expect } from "vitest";
import { ansiToHtml, stripAnsi } from "../ansiToHtml";

describe("ansiToHtml", () => {
  it("wraps a colored segment in a span with a style attribute", () => {
    const html = ansiToHtml("\x1b[31mred\x1b[0m");
    expect(html).toContain("<span");
    expect(html).toContain("red");
    expect(html).toContain("style=");
  });
  it("stripAnsi removes escape codes", () => {
    expect(stripAnsi("\x1b[31mred\x1b[0m")).toBe("red");
  });
});
