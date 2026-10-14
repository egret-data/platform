import { describe, expect, it } from "vitest";
import {
  baseName,
  checkName,
  classifyBytes,
  fileKindFor,
  imageMime,
  isUnder,
  joinPath,
  languageFor,
  movedPath,
  parentDir,
  renamedPath,
  sortChildren,
} from "@/components/workflows/namespaces/files/nsFilePaths";

describe("namespace file paths", () => {
  it("names, parents and joins W9b's stored paths (a leading /; a directory ends with /)", () => {
    expect(baseName("/scripts/etl/load.py")).toBe("load.py");
    expect(baseName("/scripts/etl/")).toBe("etl");
    expect(parentDir("/scripts/etl/load.py")).toBe("/scripts/etl/");
    expect(parentDir("/scripts/etl/")).toBe("/scripts/");
    expect(parentDir("/a.py")).toBe("/");
    expect(joinPath("/scripts/", "a.py")).toBe("/scripts/a.py");
    expect(joinPath("/", "x/y.sql")).toBe("/x/y.sql");
    expect(joinPath("/", "lib", true)).toBe("/lib/");
  });

  it("tells whether a path is a directory itself or under it", () => {
    expect(isUnder("/a/b.py", "/a/")).toBe(true);
    expect(isUnder("/a/", "/a/")).toBe(true);
    expect(isUnder("/ab/c", "/a/")).toBe(false);
    expect(isUnder("/a/b.py", "/")).toBe(true);
  });

  it("moves a file or a folder into a folder, keeping its name", () => {
    expect(movedPath("/a/x.py", "/b/")).toBe("/b/x.py");
    expect(movedPath("/a/lib/", "/")).toBe("/lib/");
    expect(movedPath("/lib/", "/b/c/")).toBe("/b/c/lib/");
  });

  it("renames in place", () => {
    expect(renamedPath("/a/x.py", "y.py")).toBe("/a/y.py");
    expect(renamedPath("/a/lib/", "src")).toBe("/a/src/");
  });

  it("refuses an empty name, a dot segment and, for a rename, a slash; the server judges the rest", () => {
    expect(checkName("", { nested: true })).toMatch(/Enter a name/);
    expect(checkName("  ", { nested: true })).toMatch(/Enter a name/);
    expect(checkName("a/../b", { nested: true })).toMatch(/"\." or "\.\."/);
    expect(checkName("a//b", { nested: true })).toMatch(/empty folder name/);
    expect(checkName("a/b", { nested: false })).toMatch(/cannot contain \//);
    expect(checkName("a/b.py", { nested: true })).toBeNull();
    // `_files` is W9b's rule: the server's message says it, not the form.
    expect(checkName("_files/x", { nested: true })).toBeNull();
  });

  it("lists folders first, then files, each by name", () => {
    const sorted = sortChildren([
      { path: "/b.py", type: "File", size: 1, revision: 1 },
      { path: "/Zeta/", type: "Directory", size: 0, revision: 1 },
      { path: "/a.py", type: "File", size: 1, revision: 1 },
      { path: "/alpha/", type: "Directory", size: 0, revision: 1 },
    ]);
    expect(sorted.map((e) => e.path)).toEqual(["/alpha/", "/Zeta/", "/a.py", "/b.py"]);
  });
});

describe("what a file opens as", () => {
  it("takes Monaco's language from the extension", () => {
    expect(languageFor("/a/load.py")).toBe("python");
    expect(languageFor("/q.SQL")).toBe("sql");
    expect(languageFor("/flow.yml")).toBe("yaml");
    expect(languageFor("/x.yaml")).toBe("yaml");
    expect(languageFor("/run.sh")).toBe("shell");
    expect(languageFor("/README.md")).toBe("markdown");
    expect(languageFor("/conf.json")).toBe("json");
    expect(languageFor("/Dockerfile")).toBe("dockerfile");
    expect(languageFor("/notes")).toBe("plaintext");
    expect(languageFor("/data.csv")).toBe("plaintext");
  });

  it("knows images and binaries by extension; anything else is read and judged by its bytes", () => {
    expect(fileKindFor("/logo.png")).toBe("image");
    expect(fileKindFor("/pic.JPEG")).toBe("image");
    expect(fileKindFor("/icon.svg")).toBe("image");
    expect(fileKindFor("/bundle.zip")).toBe("binary");
    expect(fileKindFor("/lib.jar")).toBe("binary");
    expect(fileKindFor("/t.parquet")).toBe("binary");
    expect(fileKindFor("/a.py")).toBe("text");
    expect(fileKindFor("/unknown.ext")).toBe("text");
    expect(imageMime("/icon.svg")).toBe("image/svg+xml");
    expect(imageMime("/p.jpg")).toBe("image/jpeg");
  });

  it("reads UTF-8 text, keeping a byte-order mark, and calls invalid UTF-8 or a NUL byte binary", () => {
    expect(classifyBytes(new TextEncoder().encode("héllo\r\n"))).toEqual({ kind: "text", text: "héllo\r\n" });
    const bom = new Uint8Array([0xef, 0xbb, 0xbf, 0x61]);
    expect(classifyBytes(bom)).toEqual({ kind: "text", text: "﻿a" });
    expect(classifyBytes(new Uint8Array([0x61, 0xff, 0x62])).kind).toBe("binary");
    expect(classifyBytes(new Uint8Array([0x61, 0x00, 0x62])).kind).toBe("binary");
  });
});
