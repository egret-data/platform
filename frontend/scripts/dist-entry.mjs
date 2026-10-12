/**
 * WHICH CHUNK IN `dist/assets` IS THE ENTRY BUNDLE?
 *
 * Both bundle guards need to name the entry chunk, and both used to find it with
 * `/^index-.*\.js$/`. THAT REGEX IS NOT A TEST FOR "ENTRY" — it is a test for "is called index",
 * and Rollup names a lazy chunk after the module file it was split at. `write-excel-file`'s browser
 * build lives at `browser/index.js`, so the moment `export-xlsx.ts` code-split it the assets
 * directory held TWO `index-<hash>.js` files and both guards died with "`dist/` is stale" against a
 * bundle that had just been built. A guard that fails on a correct build is worse than no guard:
 * the next person deletes the step.
 *
 * `dist/index.html` names the entry unambiguously — it is the one file the browser is told to
 * load — so that is what this reads. Any number of chunks may now be called `index-*.js` without
 * confusing anything.
 */
import { readFileSync } from "node:fs";
import { join } from "node:path";

/**
 * @param {string} dist  path to `dist/`
 * @returns {string} the entry chunk's basename within `dist/assets`
 * @throws {Error} with a message worth printing when `dist/` is missing or has no module script
 */
export function entryChunkName(dist) {
  let html;
  try {
    html = readFileSync(join(dist, "index.html"), "utf8");
  } catch {
    throw new Error(`no ${join(dist, "index.html")} — run \`npm run build\` first. This check reads the production bundle; there is nothing to assert without one.`);
  }
  const matches = [...html.matchAll(/<script[^>]+type="module"[^>]+src="\/assets\/([^"]+\.js)"/g)].map((m) => m[1]);
  if (matches.length !== 1) {
    throw new Error(`dist/index.html names ${matches.length} module entry scripts (${matches.join(", ") || "none"}); expected exactly one. Has the Vite build config grown a second input?`);
  }
  return matches[0];
}

/**
 * The chunks `dist/index.html` preloads beside the entry (`<link rel="modulepreload">`): the
 * entry's static imports, which the browser downloads on every page just as it does the entry.
 * Rollup moves a module the entry imports statically into one of these, instead of into the entry
 * itself, when a lazy chunk imports it too, so "not in the entry chunk" alone does not mean lazy
 * (W7b Task 3: an eager import of `executions/polling.ts` landed in `polling-<hash>.js`, preloaded,
 * and the entry-only check passed).
 *
 * @param {string} dist  path to `dist/`
 * @returns {string[]} basenames within `dist/assets`
 */
export function entryPreloadNames(dist) {
  const html = readFileSync(join(dist, "index.html"), "utf8");
  // Attribute order is Vite's to choose, so each `<link>` is read whole and its `rel` and `href`
  // asked for separately (W7b Task 3 review, M-1): a regex that wanted `rel` first would answer
  // `[]` for `<link href=… rel="modulepreload">`, and the guard would fall back to entry-only.
  const names = [];
  for (const [tag] of html.matchAll(/<link\b[^>]*>/g)) {
    if (!/\brel="modulepreload"/.test(tag)) continue;
    const href = /\bhref="\/assets\/([^"]+\.js)"/.exec(tag);
    if (href) names.push(href[1]);
  }
  return names;
}
