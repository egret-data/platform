import { describe, expect, it } from "vitest";
import {
  type PropType,
  trinoCatalog,
  trinoCatalogByKey,
} from "./trinoCatalog";

const VALID_TYPES: PropType[] = [
  "string",
  "number",
  "boolean",
  "enum",
  "size",
  "duration",
];
const VALID_FILES = ["config", "node", "log"] as const;

describe("trinoCatalog", () => {
  it("has a substantial number of extracted properties", () => {
    expect(trinoCatalog.length).toBeGreaterThan(150);
  });

  it("every entry has non-empty key/group/help", () => {
    for (const p of trinoCatalog) {
      expect(p.key.trim(), `key for ${JSON.stringify(p)}`).not.toBe("");
      expect(p.group.trim(), `group for ${p.key}`).not.toBe("");
      expect(p.help.trim(), `help for ${p.key}`).not.toBe("");
    }
  });

  it("every entry has a valid type and file", () => {
    for (const p of trinoCatalog) {
      expect(VALID_TYPES, `type for ${p.key}`).toContain(p.type);
      expect(VALID_FILES, `file for ${p.key}`).toContain(p.file);
    }
  });

  it("enum entries carry enumOptions and default (if any) is a valid option", () => {
    for (const p of trinoCatalog) {
      if (p.type === "enum") {
        expect(p.enumOptions?.length, `enumOptions for ${p.key}`).toBeGreaterThan(
          0,
        );
        if (p.default !== undefined) {
          expect(p.enumOptions, `default for ${p.key}`).toContain(p.default);
        }
      }
    }
  });

  it("keys are unique", () => {
    const keys = trinoCatalog.map((p) => p.key);
    expect(new Set(keys).size).toBe(keys.length);
  });

  it("trinoCatalogByKey indexes every property", () => {
    expect(Object.keys(trinoCatalogByKey).length).toBe(trinoCatalog.length);
    expect(trinoCatalogByKey["query.max-memory"]?.type).toBe("size");
    expect(trinoCatalogByKey["spill-enabled"]?.type).toBe("boolean");
    expect(trinoCatalogByKey["node.environment"]?.file).toBe("node");
  });
});
