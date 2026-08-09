#!/usr/bin/env node
/**
 * IS THE `.xlsx` WRITER IN ITS OWN CHUNK, AND IN A CHUNK AT ALL?
 *
 * `export-xlsx.ts` reaches `write-excel-file` through `await import(...)`, so Rollup should emit it
 * as a separate chunk that no dashboard fetches until someone picks `.xlsx`. One static import
 * anywhere — an editor auto-import, a "tidy-up" of the dynamic one, a type import written without
 * `import type` — collapses that back into the entry bundle, and NOTHING ELSE IN THIS REPO WOULD
 * NOTICE:
 *
 *   NEITHER VITEST NOR `vite dev` TREE-SHAKES OR CODE-SPLITS. Every unit test passes with the
 *   library eager, lazy, or duplicated. The property this asserts exists only in a production
 *   build, so only a production build can be asked about it. This is the same reason
 *   `assert-label-layout-bundled.mjs` exists, and that one was written after the repo shipped a
 *   build where six files' options were dead code.
 *
 * WHY THIS MARKER. `writeXlsxFile`, `generateXlsxFileContents` and `zipToArrayBuffer` are local
 * bindings and are mangled to nothing by the minifier. `xl/worksheets/sheet` is a STRING LITERAL
 * the writer concatenates a sheet id onto (`xlsx/generateXlsxFileContents.js:118`), so it survives
 * minification verbatim and cannot be produced by anything else in this app.
 *
 * TWO ASSERTIONS, AND THE SECOND MATTERS AS MUCH AS THE FIRST. "Absent from the entry chunk" is
 * also true of a build where the import was deleted outright and `.xlsx` downloads throw — so the
 * marker must be absent from the entry AND present in exactly one other chunk.
 */
import { readdirSync, readFileSync, statSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const ASSETS = join(dirname(fileURLToPath(import.meta.url)), "..", "dist", "assets");
const MARKER = "xl/worksheets/sheet";

function die(message) {
  console.error(`\n  ✗ ${message}\n`);
  process.exit(1);
}

let files;
try {
  files = readdirSync(ASSETS).filter((f) => f.endsWith(".js"));
} catch {
  die(`no ${ASSETS} — run \`npm run build\` first. This check reads the production bundle; there is nothing to assert without one.`);
}

const entries = files.filter((f) => /^index-.*\.js$/.test(f));
if (entries.length === 0) die("no dist/assets/index-*.js — run `npm run build` first.");
if (entries.length > 1) {
  // A stale bundle beside a fresh one has produced a wrong conclusion twice in this programme.
  die(`${entries.length} candidate entry bundles (${entries.join(", ")}) — \`dist/\` is stale. Remove it and rebuild.`);
}
const entry = entries[0];

const carriers = files.filter((f) => readFileSync(join(ASSETS, f), "utf8").includes(MARKER));
const size = (f) => statSync(join(ASSETS, f)).size;

if (carriers.includes(entry)) {
  die(
    `${entry} (${size(entry)} bytes) contains "${MARKER}" — the .xlsx writer is in the ENTRY bundle.\n`
    + `    Every dashboard visitor now downloads write-excel-file + fflate to render a chart.\n`
    + `    Check that src/components/chart/export-xlsx.ts still reaches it only through \`await import(...)\`,\n`
    + `    and that nothing imports write-excel-file statically.`,
  );
}
if (carriers.length === 0) {
  die(
    `no chunk in dist/assets contains "${MARKER}" — the .xlsx writer is in NO bundle at all.\n`
    + `    A .xlsx download will throw at runtime. Check that downloadXlsx() is still reachable\n`
    + `    from ChartExportMenu, and that the import specifier is "write-excel-file/browser".`,
  );
}
if (carriers.length > 1) {
  die(`"${MARKER}" appears in ${carriers.length} chunks (${carriers.join(", ")}) — the writer is duplicated.`);
}

console.log(
  `  ✓ ${entry} (${size(entry)} bytes) carries no .xlsx writer; `
  + `write-excel-file is lazy in ${carriers[0]} (${size(carriers[0])} bytes)`,
);
