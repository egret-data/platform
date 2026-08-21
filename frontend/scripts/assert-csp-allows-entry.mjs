#!/usr/bin/env node
/**
 * WOULD THE SHIPPED CSP LET THE SHIPPED `index.html` BOOT?
 *
 * `nginx-prod.conf` sends `script-src 'self'` with no `'unsafe-inline'`. That is only survivable
 * because Vite emits the entry as an external module tag and nothing else. If a plugin, a
 * `transformIndexHtml` hook, or a hand-edit ever puts an inline `<script>` or a cross-origin
 * `src` into `index.html`, the browser refuses to run it and the user gets a WHITE PAGE — while
 * `npm test`, `vite dev` and `vite preview` all stay green, because none of them serves the CSP.
 * That is the exact failure shape this repo already learned the hard way with the Monaco CDN: a
 * production-only property that no unit test can observe.
 *
 * So this reads the built artefact and asserts the two things the policy demands of it:
 *   1. no inline <script> (a <script> with no src, ignoring non-executable type=... blocks);
 *   2. every executable src, and every modulepreload href, is a same-origin ROOT-RELATIVE path.
 *
 * It cannot run under vitest: vitest does not produce `dist/`. It belongs beside
 * `assert-label-layout-bundled.mjs` and `assert-xlsx-lazy.mjs` in CI, right after `npm run build`.
 *
 * It deliberately does NOT parse the nginx file. The pairing it guards is "the build stayed
 * CSP-clean"; that the header itself still says `script-src 'self'` with no `'unsafe-*'` is
 * pinned by `src/lib/__tests__/csp.test.ts`, which can read the config without a build.
 */
import { readFileSync } from "node:fs";
import { join, dirname } from "node:path";
import { fileURLToPath } from "node:url";

const DIST = join(dirname(fileURLToPath(import.meta.url)), "..", "dist");
const INDEX = join(DIST, "index.html");

function die(message) {
  console.error(`\n  ✗ ${message}\n`);
  process.exit(1);
}

let html;
try {
  html = readFileSync(INDEX, "utf8");
} catch {
  die(`no ${INDEX} — run \`npm run build\` first. This check reads the production bundle; there is nothing to assert without one.`);
}

const tags = [...html.matchAll(/<script\b([^>]*)>/gi)].map((m) => m[1]);

// A <script type="application/json"> or type="importmap" is data, not code, and `script-src`
// treats it as such. Only executable blocks matter here.
const EXECUTABLE = (attrs) => {
  const type = /\btype\s*=\s*"([^"]*)"/i.exec(attrs)?.[1]?.trim().toLowerCase();
  return !type || type === "module" || type === "text/javascript" || type === "application/javascript";
};

const inline = tags.filter((a) => EXECUTABLE(a) && !/\bsrc\s*=/i.test(a));
if (inline.length > 0) {
  die(
    `dist/index.html has ${inline.length} inline <script> block(s), which \`script-src 'self'\` blocks:\n` +
      inline.map((a) => `      <script${a}>`).join("\n") +
      `\n\n    In production this is a blank page, not a warning. Either emit the code as a file, or\n` +
      `    decide deliberately to weaken the CSP — but do not discover it from a bug report.`,
  );
}

// `//host/x.js` is protocol-relative and cross-origin; `https://…` obviously so. Only a path
// beginning with a single `/` is same-origin under `'self'`.
const SAME_ORIGIN = (url) => url.startsWith("/") && !url.startsWith("//");

// Single OR double quotes, and unquoted. Matching only `src="…"` silently DROPPED a
// single-quoted cross-origin src from `srcs` while the loose inline test also let it past,
// so it counted as neither foreign nor inline. Vite emits double quotes, so that was latent
// rather than live — but a guard that depends on the emitter's quoting style is pinned to
// the emitter, not to the property.
const ATTR = (name) => new RegExp(`\\b${name}\\s*=\\s*(?:"([^"]*)"|'([^']*)'|([^\\s>]+))`, "i");
const attr = (tag, name) => {
  const m = ATTR(name).exec(tag);
  return m ? (m[1] ?? m[2] ?? m[3]) : undefined;
};

const srcs = tags.filter(EXECUTABLE).map((a) => attr(a, "src")).filter(Boolean);
// `prefetch`/`preload as=script` fetch executable code too, and `script-src` governs them.
const preloads = [...html.matchAll(/<link\b[^>]*>/gi)]
  .map((m) => m[0])
  .filter((tag) => {
    const rel = (attr(tag, "rel") || "").toLowerCase();
    if (rel === "modulepreload") return true;
    return (rel === "preload" || rel === "prefetch") && (attr(tag, "as") || "").toLowerCase() === "script";
  })
  .map((tag) => attr(tag, "href"))
  .filter(Boolean);

const foreign = [...srcs, ...preloads].filter((u) => !SAME_ORIGIN(u));
if (foreign.length > 0) {
  die(
    `dist/index.html loads script(s) from outside our own origin, which \`script-src 'self'\` blocks:\n` +
      foreign.map((u) => `      ${u}`).join("\n"),
  );
}

if (srcs.length === 0) {
  die(`dist/index.html loads no script at all. Either the build is broken or this check is reading the wrong file — both are worth stopping for.`);
}

console.log(`  ✓ dist/index.html is loadable under \`script-src 'self'\`: ${srcs.length} external script(s), ${preloads.length} modulepreload(s), 0 inline.`);
