import { describe, expect, it } from "vitest";
import type { ExistingMask, RuleEffect } from "@/api/generated/model";
import { effectSentence, effectivePerPrincipal, notEnforcedWarning, partlyEnforcedLines } from "@/routes/governance/ruleEffect";

const base: RuleEffect = {
  kind: "mask", catalogs: [], tableCount: 1, columnCount: 1, enforcedBy: 2, exemptMembers: 3, exemptEveryone: false, alreadyMasked: [], truncated: false,
};

describe("effectSentence", () => {
  it("says what is masked, by how many engines, for how many people", () => {
    expect(effectSentence(base)).toBe("Masks 1 column on 1 table · enforced by 2 engines · 3 people exempt");
    expect(effectSentence({ ...base, columnCount: 12, tableCount: 5, enforcedBy: 1, exemptMembers: 1 }))
      .toBe("Masks 12 columns on 5 tables · enforced by 1 engine · 1 person exempt");
    expect(effectSentence({ ...base, columnCount: null })).toBe("Masks every column of 1 table · enforced by 2 engines · 3 people exempt");
    expect(effectSentence({ ...base, tableCount: null, columnCount: null })).toBe("Masks the columns matching the path · enforced by 2 engines · 3 people exempt");
    expect(effectSentence({ ...base, kind: "row_filter", tableCount: 4, columnCount: null, exemptMembers: 0 }))
      .toBe("Filters rows of 4 tables · enforced by 2 engines · nobody exempt");
    expect(effectSentence({ ...base, exemptEveryone: true })).toBe("Masks 1 column on 1 table · enforced by 2 engines · everyone exempt");
  });

  it("leaves the engine count out when nothing enforces it (the warning says it)", () => {
    expect(effectSentence({ ...base, enforcedBy: 0 })).toBe("Masks 1 column on 1 table · 3 people exempt");
  });

  it("never prints null: an absent count reads as the path, and a reach of nothing says so", () => {
    expect(effectSentence({ ...base, kind: "row_filter", tableCount: null, columnCount: null }))
      .toBe("Filters rows of the tables matching the path · enforced by 2 engines · 3 people exempt");
    expect(effectSentence({ ...base, tableCount: undefined, columnCount: undefined }))
      .toBe("Masks the columns matching the path · enforced by 2 engines · 3 people exempt");
    expect(effectSentence({ ...base, tableCount: null, columnCount: 3 }))
      .toBe("Masks 3 columns on the tables matching the path · enforced by 2 engines · 3 people exempt");
    expect(effectSentence({ ...base, tableCount: 0, columnCount: 0, enforcedBy: 0 })).toBe("Masks no column yet · 3 people exempt");
    expect(effectSentence({ ...base, kind: "row_filter", tableCount: 0, columnCount: null, enforcedBy: 0 }))
      .toBe("Filters no table yet · 3 people exempt");
  });
});

describe("notEnforcedWarning (spec C7.2)", () => {
  const cat = (catalogName: string, notEnforced: "no_engine" | "no_profile" | "no_tag_service" | null) => ({
    catalogId: catalogName, catalogName, engines: [], notEnforced,
  });

  it("is null while an engine enforces the rule", () => {
    expect(notEnforcedWarning(base)).toBeNull();
  });

  it("names the catalogs without an engine", () => {
    expect(notEnforcedWarning({ ...base, enforcedBy: 0, notEnforced: "no_engine", catalogs: [cat("wh", "no_engine")] })).toEqual([
      "wh has no engine attached, so this rule is stored but no query is masked. Attach an engine in Control Plane > Engines.",
    ]);
    expect(
      notEnforcedWarning({ ...base, kind: "row_filter", enforcedBy: 0, notEnforced: "no_engine", catalogs: [cat("a", "no_engine"), cat("b", "no_engine")] }),
    ).toEqual(["a and b have no engine attached, so this rule is stored but no query is filtered. Attach an engine in Control Plane > Engines."]);
  });

  it("says when engines exist but no policy profile covers them", () => {
    expect(notEnforcedWarning({ ...base, enforcedBy: 0, notEnforced: "no_profile", catalogs: [cat("wh", "no_profile"), cat("lake", "no_engine")] })).toEqual([
      "wh's engines are not in a policy profile, so this rule is stored but no query is masked. Its engines have no policy profile; bind one in Governance > Engine & policy.",
      "lake has no engine attached, so this rule is stored but no query is masked. Attach an engine in Control Plane > Engines.",
    ]);
    expect(notEnforcedWarning({ ...base, enforcedBy: 0, notEnforced: "no_profile", catalogs: [cat("a", "no_profile"), cat("b", "no_profile")] })).toEqual([
      "The engines of a and b are not in a policy profile, so this rule is stored but no query is masked. Their engines have no policy profile; bind one in Governance > Engine & policy.",
    ]);
  });

  it("says when the engines' policy profiles have no tag service for a classification rule", () => {
    expect(
      notEnforcedWarning({ ...base, enforcedBy: 0, notEnforced: "no_tag_service", catalogs: [cat("wh", "no_tag_service"), cat("lake", "no_engine")] }),
    ).toEqual([
      "wh's engines are in a policy profile with no tag service, so this rule is stored but no query is masked. Classification rules reach a Trino engine only through its profile's tag service; see the profile in Governance > Engine & policy.",
      "lake has no engine attached, so this rule is stored but no query is masked. Attach an engine in Control Plane > Engines.",
    ]);
    expect(notEnforcedWarning({ ...base, enforcedBy: 0, notEnforced: "no_tag_service", catalogs: [cat("a", "no_tag_service"), cat("b", "no_tag_service")] })).toEqual([
      "The engines of a and b are in a policy profile with no tag service, so this rule is stored but no query is masked. Classification rules reach a Trino engine only through its profile's tag service; see the profile in Governance > Engine & policy.",
    ]);
  });

  it("says when the classification is attached to nothing yet, and only for a classification rule", () => {
    expect(notEnforcedWarning({ ...base, enforcedBy: 0, notEnforced: "no_assignment", tableCount: 0, columnCount: 0 })).toEqual([
      "This classification is not attached to any table or column a catalog serves yet, so this rule is stored but no query is masked. Attach it in Governance > Data.",
    ]);
    expect(notEnforcedWarning({ ...base, enforcedBy: 0, notEnforced: "no_assignment" }, "path")).toEqual([
      "No catalog egret knows serves this path yet, so this rule is stored but no query is masked.",
    ]);
  });

  it("falls back to a plain sentence when no catalog names a reason it knows", () => {
    expect(notEnforcedWarning({ ...base, enforcedBy: 0, notEnforced: null, catalogs: [cat("wh", null)] })).toEqual([
      "No engine enforces it yet, so this rule is stored but no query is masked.",
    ]);
  });
});

const g = (ref: string) => ({ kind: "group" as const, ref });
const other = (name: string, maskType: string, include: ReturnType<typeof g>[]): ExistingMask => ({
  ruleId: name, ruleName: name, maskType, coordinate: "c.s.t.email", exempt: { include, exclude: [] },
});

describe("effectivePerPrincipal (spec C6)", () => {
  it("exempt from every mask sees the value; from none is masked; from some is undecided", () => {
    const lines = effectivePerPrincipal(
      { maskType: "MASK", exempt: { include: [g("data-team"), g("auditors")], exclude: [] } },
      [other("Hash personal data", "MASK_HASH", [g("data-stewards"), g("auditors")])],
    );
    expect(lines.map((l) => [l.principal, l.verdict])).toEqual([
      ["group auditors", "real"],
      ["group data-stewards", "undecided"],
      ["group data-team", "undecided"],
      ["Everyone else", "masked"],
    ]);
    expect(lines.find((l) => l.principal === "Everyone else")?.text).toBe("masked; which mask applies is decided by engine order");
    expect(lines[0].text).toBe("sees the real value");
    // A cut list cannot promise the real value: it holds only the masks listed.
    expect(effectivePerPrincipal(
      { maskType: "MASK", exempt: { include: [g("auditors")], exclude: [] } },
      [other("A", "MASK", [g("auditors")])],
      { truncated: true },
    )[0].text).toBe("sees the real value (of the masks listed)");
    expect(lines[1].text).toBe("undecided: the engine applies whichever mask it evaluates first");
  });

  it("names the mask type when every mask agrees", () => {
    const lines = effectivePerPrincipal({ maskType: "MASK_HASH", exempt: { include: [], exclude: [] } }, [other("A", "MASK_HASH", [])]);
    expect(lines).toEqual([{ principal: "Everyone else", verdict: "masked", text: "masked (Hash)" }]);
  });

  it("two custom masks never count as agreeing: the preview carries no expression to compare", () => {
    const lines = effectivePerPrincipal({ maskType: "CUSTOM", exempt: { include: [], exclude: [] } }, [other("A", "CUSTOM", [])]);
    expect(lines).toEqual([{ principal: "Everyone else", verdict: "masked", text: "masked; which mask applies is decided by engine order" }]);
  });

  it("a principal no mask exempts but one named twice is listed once; a type not listed shows as stored", () => {
    const lines = effectivePerPrincipal(
      { maskType: "MASK_ODD", exempt: { include: [{ kind: "user", ref: "zoe" }], exclude: [] } },
      [other("A", "MASK_ODD", [{ kind: "user", ref: "zoe" } as never]), other("B", "MASK_ODD", [])],
    );
    expect(lines).toEqual([
      { principal: "user zoe", verdict: "undecided", text: "undecided: the engine applies whichever mask it evaluates first" },
      { principal: "Everyone else", verdict: "masked", text: "masked (MASK_ODD)" },
    ]);
  });
});

describe("partlyEnforcedLines (live check D2)", () => {
  const cat = (catalogName: string, notEnforced: RuleEffect["notEnforced"], engines = notEnforced ? [] : [{ id: "e-1", name: "trino-a", engineType: "trino" }]) => ({
    catalogId: `c-${catalogName}`,
    catalogName,
    engines,
    notEnforced,
  });

  it("names each catalog not enforced, grouped by the existing reason phrase", () => {
    expect(
      partlyEnforcedLines(
        { enforcedBy: 1, catalogs: [cat("sales", null), cat("demo_warehouse", "no_engine"), cat("lake", "no_engine"), cat("ops", "no_profile")] },
        "tag",
      ),
    ).toEqual([
      "Not enforced in demo_warehouse and lake — no engine attached.",
      "Not enforced in ops — its engines have no policy profile.",
    ]);
  });

  it("says nothing when every catalog is enforced, or when no catalog is (the Not enforced yet warning says that)", () => {
    expect(partlyEnforcedLines({ enforcedBy: 1, catalogs: [cat("sales", null)] }, "tag")).toBeNull();
    expect(partlyEnforcedLines({ enforcedBy: 0, catalogs: [cat("demo_warehouse", "no_engine")] }, "tag")).toBeNull();
    expect(partlyEnforcedLines({ enforcedBy: 2, catalogs: [] }, "path")).toBeNull();
  });
});
