// @vitest-environment node
import { describe, it, expect } from "vitest";
import { resolve, dirname } from "node:path";
import { fileURLToPath } from "node:url";
import { ESLint } from "eslint";

const cwd = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const eslint = new ESLint({ cwd });
const severity = (entry) => (Array.isArray(entry) ? entry[0] : entry);

// Loading the whole ESLint config cold takes seconds on a busy machine; the
// default 5 s timed out under load (final re-review).
describe("jsx-a11y configuration", { timeout: 30_000 }, () => {
  it("turns the recommended rules on as errors for source components", async () => {
    const config = await eslint.calculateConfigForFile("src/components/explore/EmptyState.tsx");
    expect(severity(config.rules["jsx-a11y/alt-text"])).toBe(2);
    expect(severity(config.rules["jsx-a11y/label-has-associated-control"])).toBe(2);
  });

  it("leaves no-autofocus to review (FE-A11Y-6)", async () => {
    const config = await eslint.calculateConfigForFile("src/components/explore/EmptyState.tsx");
    expect(severity(config.rules["jsx-a11y/no-autofocus"])).toBe(0);
  });

  it("allows role=list on a list, which Safari needs, and still refuses other redundant roles", async () => {
    const lint = async (code) => {
      const [result] = await eslint.lintText(code, { filePath: "src/components/explore/Probe.tsx" });
      return result.messages.filter((m) => m.ruleId === "jsx-a11y/no-redundant-roles");
    };
    expect(await lint('export const A = () => <ul role="list"><li>a</li></ul>;')).toEqual([]);
    expect(await lint('export const A = () => <ol role="list"><li>a</li></ol>;')).toEqual([]);
    expect(await lint('export const A = () => <button role="button">a</button>;')).toHaveLength(1);
  });

  it("lets the rules see through egret's primitives", async () => {
    const lint = async (code) => {
      const [result] = await eslint.lintText(code, { filePath: "src/components/explore/Probe.tsx" });
      return result.messages.filter((m) => m.ruleId === "jsx-a11y/label-has-associated-control");
    };
    expect(await lint("export const A = () => <Label>Name</Label>;")).toHaveLength(1);
    expect(
      await lint('export const A = () => <><Label htmlFor="x">Name</Label><Input id="x" /></>;'),
    ).toEqual([]);
  });

  it("leaves test files alone", async () => {
    const config = await eslint.calculateConfigForFile("src/components/explore/__tests__/x.test.tsx");
    const on = Object.entries(config.rules ?? {}).filter(
      ([id, entry]) => id.startsWith("jsx-a11y/") && severity(entry) !== 0,
    );
    expect(on).toEqual([]);
    // The test file must not be vacuously clean: the rules exist for source files.
    const src = await eslint.calculateConfigForFile("src/components/explore/EmptyState.tsx");
    expect(severity(src.rules["jsx-a11y/alt-text"])).toBe(2);
  });
});
