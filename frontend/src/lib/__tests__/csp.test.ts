import { describe, it, expect } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";

/**
 * WHAT THIS PINS, AND WHAT IT CANNOT.
 *
 * `nginx-prod.conf` is the only place egret sends a Content-Security-Policy, and the directive
 * that matters most there — `script-src` — exists to make a repeat of the Monaco CDN defect
 * FAIL rather than run. `@monaco-editor/loader` still ships `https://cdn.jsdelivr.net/npm/...`
 * as its default in our bundle; the code that overrides it is a line of TypeScript, and a
 * refactor can undo a line of TypeScript. The header is the layer that does not care.
 *
 * This test can only assert the STRING. It cannot tell you the browser enforces it, and it
 * cannot tell you the app still works under it — both of those were measured by serving the
 * built `dist` under this exact header and diffing against the same `dist` served without it.
 * What this test is for is the other half: that nobody quietly widens the policy back open,
 * which is invisible in a diff review of a 3,000-character line.
 */
const NGINX = resolve(__dirname, "../../../nginx-prod.conf");

/**
 * The body of the SPA's `location / { … }`, brace-matched.
 *
 * The header has to be found HERE and nowhere else, because nginx's `add_header`
 * inheritance is REPLACE, not merge: a level that declares any `add_header` drops
 * every one it would otherwise inherit. So "tidying the security headers up to the
 * `server` level" — leaving `X-Frame-Options` and `X-Content-Type-Options` behind in
 * `location /` — silently deletes the CSP from every SPA response while `nginx -t`
 * stays happy. Measured: real nginx serving the real `dist` then returns
 * `X-Frame-Options` and `X-Content-Type-Options` and NO `Content-Security-Policy`.
 *
 * A file-wide regex cannot see that, which is the point. This guard was written to
 * stop the policy being widened; scoping it is what also makes it notice the policy
 * being deleted.
 */
function spaLocationBlock(): string {
  const conf = readFileSync(NGINX, "utf8");
  const open = conf.search(/^\s*location\s+\/\s*\{/m);
  expect(open, "expected a `location / {` block in nginx-prod.conf").toBeGreaterThan(-1);
  let depth = 0;
  for (let i = conf.indexOf("{", open); i < conf.length; i++) {
    if (conf[i] === "{") depth++;
    else if (conf[i] === "}" && --depth === 0) return conf.slice(open, i);
  }
  throw new Error("unterminated `location / {` block in nginx-prod.conf");
}

/** The value of the single `Content-Security-Policy` header the SPA server sends. */
function policy(): string {
  const block = spaLocationBlock();
  const matches = [...block.matchAll(/^\s*add_header\s+Content-Security-Policy\s+"([^"]*)"\s*(always)?\s*;/gm)];
  // More than one is not "the last one wins" — nginx sends both and the browser enforces the
  // INTERSECTION, which is a different and much more surprising policy than either line reads as.
  expect(
    matches,
    "expected exactly one Content-Security-Policy add_header inside the SPA `location / {}` block",
  ).toHaveLength(1);
  // Without `always`, nginx omits the header on error responses — including the 4xx/5xx
  // pages an attacker is most interested in getting a script onto.
  expect(matches[0][2], "the Content-Security-Policy add_header must carry `always`").toBe("always");
  return matches[0][1];
}

/** `{ "script-src": ["'self'"], … }` — parsed, so reordering the directives cannot fail this. */
function directives(): Record<string, string[]> {
  return Object.fromEntries(
    policy()
      .split(";")
      .map((d) => d.trim())
      .filter(Boolean)
      .map((d) => {
        const [name, ...sources] = d.split(/\s+/);
        return [name.toLowerCase(), sources];
      }),
  );
}

describe("the production Content-Security-Policy", () => {
  it("restricts scripts to our own origin, and to nothing else", () => {
    // Not `toContain`: the point is that this is the WHOLE list. A second source added here is
    // exactly the regression — a CDN back in the allow-list — and `toContain` would sleep through it.
    expect(directives()["script-src"]).toEqual(["'self'"]);
  });

  it("allows the workers the app really builds: our own files, and blob URLs", () => {
    // `worker-src` falls back to `script-src` when absent, so dropping this line is not a
    // loosening — it silently FORBIDS `blob:`, and then a large .xlsx export and a plotly map
    // trace each hang forever with the page still rendering. Both measured; see nginx-prod.conf.
    expect(directives()["worker-src"]).toEqual(["'self'", "blob:"]);
  });

  it("grants neither 'unsafe-inline' nor 'unsafe-eval'", () => {
    // With either one present, `script-src` stops being a defence against injected code and
    // becomes decoration. Nothing in the bundle needs them: `dist/index.html` has no inline
    // script (`scripts/assert-csp-allows-entry.mjs` keeps it that way) and no chunk reaches a
    // live `eval`/`Function` call. If a future dependency does, that is a decision to argue
    // for in a review — not a token to slip into a 3,000-character line.
    expect(policy()).not.toContain("'unsafe-inline'");
    expect(policy()).not.toContain("'unsafe-eval'");
  });

  it("never lets a scheme or a wildcard stand in for an origin", () => {
    // `https:` as a source is every host on the internet that has a certificate, which is the
    // whole population of CDNs. `*` is worse. Neither reads as alarming inside a long line.
    for (const [name, sources] of Object.entries(directives())) {
      if (name === "frame-src") continue; // its own test owns that list, host by host
      for (const s of sources) {
        expect(s, `${name} has a scheme/wildcard source: ${s}`).not.toMatch(/^(https?:|\*|data:)$/);
        expect(s, `${name} has a wildcard host: ${s}`).not.toMatch(/^https?:\/\/\*$/);
      }
    }
  });

  it("still closes the two ways script provenance is subverted around script-src", () => {
    // A <base href> re-points every relative script URL; an <object data> runs a plugin
    // document. The app has neither, so both cost nothing and both are one injected tag away.
    expect(directives()["object-src"]).toEqual(["'none'"]);
    expect(directives()["base-uri"]).toEqual(["'self'"]);
  });
});
