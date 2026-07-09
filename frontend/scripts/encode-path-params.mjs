// Post-generation codemod for the orval client (E0·P4d-fix).
//
// orval v8 interpolates path params into URL templates RAW, e.g.
//   `/api/ws/${workspace}/datasets/${id}/columns/${columnName}`
// with no encodeURIComponent — a regression vs the hand-written modules, which
// encoded segments like a dataset column name. Path params sourced from
// warehouse metadata (column / namespace / table names) can legitimately
// contain spaces, `/`, `%`, `#`, so a raw name breaks or mis-routes the URL.
//
// Fix: wrap every path-segment interpolation with encodeURIComponent. We match
// ONLY `${identifier}` that is immediately preceded by a `/` — i.e. a path
// segment. This deliberately does NOT touch orval's query-string append
// (`?${stringifiedParams}`, after a `?`, not a `/`), so query params (already
// encoded via URLSearchParams) are not double-encoded. Applied to BOTH the URL
// builders and the query-key factories (same path templates), so a read's cache
// key stays byte-equal to the request URL.
//
// Idempotent: after rewriting, the interpolation becomes
// `${encodeURIComponent(id)}`, whose inner text is not a bare `\w+`, so a
// re-run does not match it again. Runs as part of `npm run gen:api`, so the
// committed generated output always reflects it (guarded by determinism).
import { readdirSync, readFileSync, writeFileSync, statSync } from "node:fs";
import { join } from "node:path";

const GENERATED_DIR = new URL("../src/api/generated", import.meta.url).pathname;
// `${ident}` preceded by `/` (path segment). Lookbehind keeps the `/` outside
// the match so it survives; the capture is the bare param name.
const PATH_PARAM = /(?<=\/)\$\{(\w+)\}/g;

let changed = 0;
function walk(dir) {
  for (const entry of readdirSync(dir)) {
    const p = join(dir, entry);
    if (statSync(p).isDirectory()) {
      walk(p);
    } else if (p.endsWith(".ts")) {
      const src = readFileSync(p, "utf8");
      const out = src.replace(PATH_PARAM, "${encodeURIComponent($1)}");
      if (out !== src) {
        writeFileSync(p, out);
        changed++;
      }
    }
  }
}

walk(GENERATED_DIR);
console.log(`encode-path-params: rewrote path-segment params in ${changed} file(s)`);
