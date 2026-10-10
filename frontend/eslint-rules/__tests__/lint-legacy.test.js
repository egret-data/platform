// @vitest-environment node
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { describe, expect, it } from "vitest";
import { staleEntries } from "../../scripts/lint-legacy.mjs";

const cwd = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const RULE = "egret/no-raw-palette";
const BREAKS = "eslint-rules/__tests__/fixtures/breaks.tsx";
const CLEAN = "eslint-rules/__tests__/fixtures/clean.tsx";

describe("staleEntries", () => {
  it("accepts a listed file that still breaks its rule", async () => {
    expect(await staleEntries({ [RULE]: [BREAKS] }, { cwd })).toEqual([]);
  });

  it("flags a listed file that is clean now", async () => {
    expect(await staleEntries({ [RULE]: [CLEAN] }, { cwd })).toEqual([
      { rule: RULE, file: CLEAN, reason: "clean" },
    ]);
  });

  it("flags a listed file that no longer exists", async () => {
    const file = "eslint-rules/__tests__/fixtures/nope.tsx";
    expect(await staleEntries({ [RULE]: [file] }, { cwd })).toEqual([
      { rule: RULE, file, reason: "missing" },
    ]);
  });

  it("throws on an unknown rule id, naming it", async () => {
    await expect(staleEntries({ "egret/no-such-rule": [BREAKS] }, { cwd })).rejects.toThrow(
      /egret\/no-such-rule/,
    );
  });

  describe("jsx-a11y rules", () => {
    const TABINDEX = "jsx-a11y/no-noninteractive-tabindex";

    it("judges a file with the main config's options (recommended allows a tabbable tabpanel)", async () => {
      const file = "eslint-rules/__tests__/fixtures/a11y-tabpanel.tsx";
      expect(await staleEntries({ [TABINDEX]: [file] }, { cwd })).toEqual([
        { rule: TABINDEX, file, reason: "clean" },
      ]);
    });

    it("accepts a listed file that still breaks the rule under those options", async () => {
      const file = "eslint-rules/__tests__/fixtures/a11y-article.tsx";
      expect(await staleEntries({ [TABINDEX]: [file] }, { cwd })).toEqual([]);
    });
  });
});
