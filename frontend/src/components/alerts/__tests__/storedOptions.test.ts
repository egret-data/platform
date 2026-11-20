import { describe, expect, it } from "vitest";

import { ownerLabel, withStoredOption } from "../storedOptions";

describe("ownerLabel", () => {
  const members = [{ principal_id: "u1", principal_name: "ann@x.io" }];
  const owners = [
    { user_id: "u1", email: "stale@x.io" },
    { user_id: "u2", email: "gone@x.io" },
  ];
  it("prefers the member's name", () => {
    expect(ownerLabel("u1", members, owners)).toBe("ann@x.io");
  });
  it("falls back to the email the alert carries, not the raw id", () => {
    expect(ownerLabel("u2", members, owners)).toBe("gone@x.io");
  });
  it("shows the id only when nothing else names the owner", () => {
    expect(ownerLabel("u3", members, owners)).toBe("u3");
  });
});

describe("withStoredOption", () => {
  const items = [
    { value: "png", label: "PNG" },
    { value: "pdf", label: "PDF" },
  ];
  it("leaves the items alone when the value is one of them", () => {
    expect(withStoredOption(items, "png", (s) => `${s} (unsupported)`)).toBe(items);
  });
  it("adds the out-of-range value as a marked option", () => {
    expect(withStoredOption(items, "csv", (s) => `${s} (unsupported)`).at(-1)).toEqual({
      value: "csv",
      label: "csv (unsupported)",
    });
  });
  it("adds nothing for an empty value", () => {
    expect(withStoredOption(items, "", () => "x")).toBe(items);
  });
});
