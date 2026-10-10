// @vitest-environment node
// Keeps docs/en/frontend-rules.md honest against the tree it describes: its ids,
// its paths, its lint rule names and its debt register's counts.
import { existsSync, readFileSync } from "node:fs";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { beforeAll, describe, expect, it } from "vitest";
import { legacy } from "../legacy.js";
import { plugins } from "../plugins.js";

const frontend = resolve(dirname(fileURLToPath(import.meta.url)), "../..");
const repo = resolve(frontend, "..");
const read = (path) => readFileSync(resolve(repo, path), "utf8");

const EN = "docs/en/frontend-rules.md";
const VI = "docs/vi/frontend-rules.md";

const GROUPS = {
  PAGE: 5,
  STATE: 4,
  ACTION: 5,
  VISUAL: 6,
  FORM: 4,
  DATA: 3,
  ROUTE: 3,
  A11Y: 6,
  COPY: 4,
  REF: 2,
  TEST: 4,
};
const EXPECTED_IDS = Object.entries(GROUPS).flatMap(([group, n]) =>
  Array.from({ length: n }, (_, i) => `FE-${group}-${i + 1}`),
);

const ID = /FE-[A-Z][A-Z0-9]*-\d+/g;
const headingIds = (text) =>
  [...text.matchAll(/^#{2,6} (FE-[A-Z][A-Z0-9]*-\d+)\b/gm)].map((m) => m[1]);

// The text from a `## <title>` heading to the next `## ` heading.
const section = (text, title) => {
  const start = text.indexOf(`\n## ${title}`);
  if (start < 0) return "";
  const next = text.indexOf("\n## ", start + 1);
  return next < 0 ? text.slice(start) : text.slice(start, next);
};

// GitHub's heading anchor: lower case, punctuation dropped, each space a hyphen.
const slug = (heading) =>
  heading
    .trim()
    .toLowerCase()
    .replace(/[^\p{L}\p{N} _-]/gu, "")
    .replace(/ /g, "-");

describe("the frontend rulebook", () => {
  // Read inside each test, so a missing file fails the tests rather than the suite.
  let en, vi;
  beforeAll(() => {
    en = existsSync(resolve(repo, EN)) ? read(EN) : "";
    vi = existsSync(resolve(repo, VI)) ? read(VI) : "";
  });

  it("gives every rule id exactly one heading, and names no other id", () => {
    const headings = headingIds(en);
    expect([...headings].sort()).toEqual([...EXPECTED_IDS].sort());
    const unknown = [...new Set(en.match(ID))].filter((id) => !EXPECTED_IDS.includes(id));
    expect(unknown).toEqual([]);
  });

  it("has the same ids in the Vietnamese reference", () => {
    expect([...headingIds(vi)].sort()).toEqual([...EXPECTED_IDS].sort());
    const unknown = [...new Set(vi.match(ID))].filter((id) => !EXPECTED_IDS.includes(id));
    expect(unknown).toEqual([]);
  });

  it("names only repo paths that exist", () => {
    const paths = [...en.matchAll(/`((?:src|frontend|docs|scripts)\/[^`\s]*)`/g)].map((m) => m[1]);
    expect(paths.length).toBeGreaterThan(10);
    const missing = paths.filter((path) => {
      const base = path.startsWith("src/") ? frontend : repo;
      return !existsSync(resolve(base, path));
    });
    expect(missing).toEqual([]);
  });

  it("lists every legacy rule in its debt register with that list's file count", () => {
    const register = section(en, "Debt register");
    const rows = register
      .split("\n")
      .filter((line) => line.startsWith("|"))
      .map((line) => line.split("|").map((cell) => cell.trim()));
    const wrong = Object.entries(legacy).flatMap(([rule, files]) => {
      const row = rows.find((cells) => cells.includes(`\`${rule}\``));
      if (!row) return [{ rule, problem: "no row" }];
      return row.includes(String(files.length)) ? [] : [{ rule, problem: `count is not ${files.length}` }];
    });
    expect(wrong).toEqual([]);
  });

  it("names only egret lint rules that exist", () => {
    const named = [...new Set([...en.matchAll(/egret\/([a-z][a-z-]*)/g)].map((m) => m[1]))];
    expect(named.length).toBeGreaterThan(0);
    const known = Object.keys(plugins.egret.rules);
    expect(named.filter((rule) => !known.includes(rule))).toEqual([]);
  });

  it("links only ui-conventions sections that exist", () => {
    const anchors = new Set(
      [...read("docs/en/ui-conventions.md").matchAll(/^#{1,6} (.+)$/gm)].map((m) => slug(m[1])),
    );
    const links = [en, vi].flatMap((text) =>
      [...text.matchAll(/\]\((?:\.\.\/en\/)?ui-conventions\.md#([^)]+)\)/g)].map((m) => m[1]),
    );
    expect(links.length).toBeGreaterThan(0);
    expect(links.filter((anchor) => !anchors.has(anchor))).toEqual([]);
  });
});

describe("the pointers to the rulebook", () => {
  it.each(["frontend/CLAUDE.md", "frontend/AGENTS.md"])("%s links the rulebook", (file) => {
    expect(existsSync(resolve(repo, file))).toBe(true);
    expect(read(file)).toContain("../docs/en/frontend-rules.md");
    expect(existsSync(resolve(repo, "frontend", "../docs/en/frontend-rules.md"))).toBe(true);
  });

  it.each(["docs/en/ui-conventions.md", "docs/vi/ui-conventions.md"])(
    "%s names the rulebook in its first five lines",
    (file) => {
      expect(read(file).split("\n").slice(0, 5).join("\n")).toContain("frontend-rules.md");
    },
  );
});
