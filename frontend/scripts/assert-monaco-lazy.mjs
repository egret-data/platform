#!/usr/bin/env node
/**
 * IS MONACO OUT OF THE ENTRY CHUNK?
 *
 * `components/monaco/MonacoEditor.tsx` reaches Monaco through `lazy(() => import("@/lib/monaco"))`
 * so Rollup emits it as its own ~3.8MB chunk that only an editor page fetches. `App.tsx` has no
 * lazy routes, so ANYTHING statically reachable from it lands in the one eager chunk the login
 * screen downloads — the difference measured at 1.01MB vs 1.95MB gzip.
 *
 * THE SOURCE-LEVEL GUARDS IN `lib/__tests__/monaco.test.ts` CANNOT SEE THIS. They match
 * `from "@monaco-editor/react"` and the `@/lib/monaco` specifier. But `SqlEditor.tsx` and
 * `notebookCompletion.ts` both carry `import type * as MonacoNS from "monaco-editor"`, and
 * DELETING THE WORD `type` — the natural move the day someone needs `MonacoNS.editor.EditorOption`
 * as a value rather than a type — is tsc-clean, lint-clean, invisible to both regexes, and puts the
 * whole Monaco graph back in the eager chunk. A deep relative path (`"../../lib/monaco"`) evades
 * them too, and so does any rename.
 *
 *   NEITHER VITEST NOR `vite dev` CODE-SPLITS. Every unit test passes with Monaco eager, lazy, or
 *   duplicated. The property exists only in a production build, so only a production build can be
 *   asked about it — the same reason `assert-xlsx-lazy.mjs` and `assert-label-layout-bundled.mjs`
 *   exist.
 *
 * WHY THESE MARKERS. `editorWorkerService` and `MonacoEnvironment` are Monaco-internal identifiers
 * that survive minification as strings (service IDs and a global hook name). Measured on a clean
 * build: 8 and 3 occurrences in the Monaco chunk, ZERO in the entry chunk.
 *
 * GUARDING THE GUARD. A marker that stops appearing anywhere would make this script pass forever
 * on any bundle, so it also asserts each marker IS present in some non-entry chunk. If a Monaco
 * upgrade renames them, this fails loudly instead of going quiet.
 *
 * MONACO-YAML (W7a Task 4, spec §5.2, FE-ROUTE-2). The flow editor's YAML language service is
 * lazier still than Monaco: `components/workflows/editor/monacoYaml.ts` is reached only through an
 * `import()` in `CodeTab.tsx`, after Monaco has loaded, and its worker is a separate file that
 * `src/lib/monaco.ts` constructs only for the label `yaml`. So both are checked against TWO chunks:
 * the entry, and the Workflows area's chunk (found by a literal of `CodeTab.tsx`), which the flow
 * list also downloads. Markers, measured on a clean build:
 *   - `monaco-yaml/yaml.worker`, the module id `configureMonacoYaml` passes to its worker manager
 *     (`monaco-yaml/index.js`): only in `monacoYaml-*.js`;
 *   - `Matches multiple schemas when only one must validate.`, a yaml-language-server message:
 *     only in `yaml.worker-*.js`.
 * Each was proven by a scratch build that imported its module eagerly (task-4-report.md).
 */
import { readFileSync, readdirSync } from "node:fs";
import { join } from "node:path";
import { entryChunkName } from "./dist-entry.mjs";

const DIST = "dist";
const ASSETS = join(DIST, "assets");
const MARKERS = ["editorWorkerService", "MonacoEnvironment"];

const entry = entryChunkName(DIST);
const entrySource = readFileSync(join(ASSETS, entry), "utf8");
const others = readdirSync(ASSETS).filter((f) => f.endsWith(".js") && f !== entry);

let failed = false;

for (const marker of MARKERS) {
  if (entrySource.includes(marker)) {
    console.error(
      `✗ ${marker} is in the entry chunk (${entry}).\n` +
        `  Monaco is eager again: every page now downloads it, including the login screen.\n` +
        `  Look for a static import of "monaco-editor" or "@monaco-editor/react" outside\n` +
        `  src/lib/monaco.ts — including an "import type" that lost its "type".`,
    );
    failed = true;
  }
  const carriers = others.filter((f) => readFileSync(join(ASSETS, f), "utf8").includes(marker));
  if (carriers.length === 0) {
    console.error(
      `✗ ${marker} appears in NO chunk at all.\n` +
        `  The marker is stale, so this guard would pass on any bundle. Pick a new one by\n` +
        `  measuring what is actually in dist/assets/monaco-*.js.`,
    );
    failed = true;
  }
}

/** `[what, marker]`: `monaco-yaml` and its worker. */
const YAML_MARKERS = [
  ["monaco-yaml (src/components/workflows/editor/monacoYaml.ts)", "monaco-yaml/yaml.worker"],
  ["monaco-yaml's worker (monaco-yaml/yaml.worker?worker in src/lib/monaco.ts)", "Matches multiple schemas when only one must validate."],
];
/** `VALIDATE_OWNER` in `CodeTab.tsx`: marks the chunk the Workflows area loads. */
const AREA_MARKER = "egret-validate";
const areaChunks = others.filter((f) => readFileSync(join(ASSETS, f), "utf8").includes(AREA_MARKER));
if (areaChunks.length === 0) {
  console.error(
    `✗ ${AREA_MARKER} (CodeTab.tsx) appears in NO lazy chunk.\n` +
      `  This guard finds the Workflows area's chunk by it; update AREA_MARKER to a literal CodeTab.tsx still has.`,
  );
  failed = true;
}
for (const [what, marker] of YAML_MARKERS) {
  if (entrySource.includes(marker)) {
    console.error(
      `✗ ${what} is in the entry chunk (${entry}).\n` +
        `  Look for a static import of "monaco-yaml" or of components/workflows/editor/monacoYaml.ts.`,
    );
    failed = true;
  }
  for (const f of areaChunks) {
    if (readFileSync(join(ASSETS, f), "utf8").includes(marker)) {
      console.error(
        `✗ ${what} is in the Workflows area's chunk (${f}).\n` +
          `  Every Workflows page now downloads it. monacoYaml.ts must be reached only through the\n` +
          `  import() in CodeTab.tsx, and the worker only through src/lib/monaco.ts.`,
      );
      failed = true;
    }
  }
  // Stale only when it is nowhere: a misplaced marker has already been reported above.
  if (!entrySource.includes(marker) && !others.some((f) => readFileSync(join(ASSETS, f), "utf8").includes(marker))) {
    console.error(
      `✗ ${marker} appears in NO chunk at all.\n` +
        `  The marker is stale, so this guard would pass on any bundle. Pick a new one by\n` +
        `  measuring what is actually in dist/assets/monacoYaml-*.js and yaml.worker-*.js.`,
    );
    failed = true;
  }
}

if (failed) process.exit(1);
console.log(
  `✓ Monaco stays out of the entry chunk (${entry}); monaco-yaml and its worker stay out of it and of the ` +
    `Workflows area's chunk; markers found in lazy chunks.`,
);
