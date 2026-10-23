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
  PAGE: 9,
  STATE: 4,
  ACTION: 5,
  VISUAL: 8,
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

// Spec §5: every rule a reviewer must confirm, because lint cannot check it or
// because its check is a test or the gate that a change must add or run.
const CHECKLIST_IDS = [
  ...["PAGE-1", "PAGE-2", "PAGE-3", "PAGE-4", "PAGE-5", "PAGE-6", "PAGE-7", "PAGE-8", "PAGE-9"],
  ...["STATE-1", "STATE-2", "STATE-3", "STATE-4"],
  ...["ACTION-2", "ACTION-3", "ACTION-4", "ACTION-5"],
  ...["VISUAL-2", "VISUAL-4", "VISUAL-5", "VISUAL-6", "VISUAL-7", "VISUAL-8"],
  ...["FORM-1", "FORM-2", "FORM-3", "FORM-4"],
  ...["DATA-1", "DATA-2", "DATA-3"],
  ...["ROUTE-1", "ROUTE-2", "ROUTE-3"],
  ...["A11Y-2", "A11Y-3", "A11Y-4", "A11Y-5", "A11Y-6"],
  ...["COPY-1", "COPY-2", "COPY-3", "COPY-4"],
  ...["REF-1", "REF-2"],
  ...["TEST-1", "TEST-2", "TEST-3", "TEST-4"],
].map((id) => `FE-${id}`);

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

  // The two files differ only in language: the same paths, counts, levels and checklist.
  const BOOKS = [
    { name: "EN", text: () => en, doThis: "**Do this:**", checkedBy: "**Checked by:**", register: "Debt register", must: "MUST", should: "SHOULD" },
    { name: "VI", text: () => vi, doThis: "**Làm thế này:**", checkedBy: "**Kiểm bằng:**", register: "Sổ nợ", must: "PHẢI", should: "NÊN" },
  ];

  const repoPaths = (text) =>
    [...text.matchAll(/`((?:src|frontend|docs|scripts)\/[^`\s]*)`/g)].map((m) => m[1]);
  const exists = (path) => existsSync(resolve(path.startsWith("src/") ? frontend : repo, path));

  // Each `#### FE-…` heading with the text up to the next heading.
  const ruleBlocks = (text) =>
    text
      .split(/^(?=#{2,4} )/m)
      .map((block) => ({ id: block.match(/^#### (FE-[A-Z][A-Z0-9]*-\d+)\b/)?.[1], block }))
      .filter(({ id }) => id);

  it.each(BOOKS)("$name names only repo paths that exist", ({ text }) => {
    const paths = repoPaths(text());
    expect(paths.length).toBeGreaterThan(10);
    expect(paths.filter((path) => !exists(path))).toEqual([]);
  });

  it.each(BOOKS)("$name gives every rule a Do this that names an existing path", ({ text, doThis, checkedBy }) => {
    const blocks = ruleBlocks(text());
    expect(blocks).toHaveLength(EXPECTED_IDS.length);
    const without = blocks
      .filter(({ block }) => {
        const at = block.indexOf(doThis);
        if (at < 0) return true;
        const end = block.indexOf(checkedBy, at);
        const paths = repoPaths(block.slice(at, end < 0 ? undefined : end));
        return !paths.some(exists);
      })
      .map(({ id }) => id);
    expect(without).toEqual([]);
  });

  it.each(BOOKS)("$name gives every rule its level: SHOULD for FE-VISUAL-5 only", ({ text, must, should }) => {
    const levels = Object.fromEntries(
      [...text().matchAll(/^#### (FE-[A-Z][A-Z0-9]*-\d+) .*· (\S+)$/gm)].map((m) => [m[1], m[2]]),
    );
    const expected = Object.fromEntries(
      EXPECTED_IDS.map((id) => [id, id === "FE-VISUAL-5" ? should : must]),
    );
    expect(levels).toEqual(expected);
  });

  it.each(BOOKS)("$name: the checklist holds exactly the rules of spec §5", ({ text }) => {
    const checklist = [...text().matchAll(/^- \[ \] (FE-[A-Z][A-Z0-9]*-\d+)\b/gm)].map((m) => m[1]);
    expect([...checklist].sort()).toEqual([...CHECKLIST_IDS].sort());
  });

  it.each(BOOKS)(
    "$name lists every legacy rule in its debt register with that list's file count",
    ({ text, register }) => {
      // | Rule | Lint rule | Files | Note | → cells[2] is the lint rule, cells[3] the count.
      const rows = section(text(), register)
        .split("\n")
        .filter((line) => line.startsWith("|"))
        .map((line) => line.split("|").map((cell) => cell.trim()));
      const wrong = Object.entries(legacy).flatMap(([rule, files]) => {
        const row = rows.find((cells) => cells[2] === `\`${rule}\``);
        if (!row) return [{ rule, problem: "no row" }];
        return row[3] === String(files.length) ? [] : [{ rule, problem: `Files is ${row[3]}, not ${files.length}` }];
      });
      expect(wrong).toEqual([]);
    },
  );

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

  // FE-TEST-4's gate, named the way the rulebook names it.
  it.each(["frontend/CLAUDE.md", "frontend/AGENTS.md"])("%s lists FE-TEST-4's whole gate", (file) => {
    const text = read(file);
    for (const step of ["npm run lint", "npx tsc -b", "npm run test", "npm run build"]) {
      expect(text).toContain(`\`${step}\``);
    }
    for (const bundle of ["npm run test:bundle", "test:bundle:xlsx", "test:bundle:csp", "test:bundle:monaco"]) {
      expect(text).toContain(`\`${bundle}\``);
    }
  });

  it.each(["docs/en/ui-conventions.md", "docs/vi/ui-conventions.md"])(
    "%s names the rulebook in its first five lines",
    (file) => {
      expect(read(file).split("\n").slice(0, 5).join("\n")).toContain("frontend-rules.md");
    },
  );
});
