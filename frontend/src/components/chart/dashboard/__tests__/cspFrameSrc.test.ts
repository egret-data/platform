import { describe, it, expect } from "vitest";
import { readFileSync } from "node:fs";
import { resolve } from "node:path";
import { ALLOWED_IFRAME_HOSTS, cspFrameSrcDirective } from "../iframe-url";

/**
 * The iframe card's allow-list is a TypeScript constant; the CSP that makes the BROWSER enforce
 * the same list is an nginx directive. Two artefacts, one rule — so they can drift, and a drift
 * is silent: the card would accept an embed the browser then blocks, or worse, the browser would
 * permit one the card rejects.
 *
 * This test is the mechanism that makes drift loud. It reads the shipped nginx config and pins it
 * against the constant. Adding a host to `ALLOWED_IFRAME_HOSTS` fails here until nginx is updated,
 * and the failure message is the line to paste.
 */
const NGINX = resolve(__dirname, "../../../../../nginx-prod.conf");

describe("the CSP frame-src and the iframe allow-list", () => {
  it("agree, host for host — plus the one deployment-supplied source, the apps origin", () => {
    const conf = readFileSync(NGINX, "utf8");
    // The apps origin is the ONLY addition: the Data Apps editor is an iframe on a sibling origin
    // (spec §6.1), and the browser refuses it unless `frame-src` names it. It is a placeholder the
    // nginx image fills in from `APPS_ORIGIN` at start, so it is pinned here as that placeholder.
    const expected = cspFrameSrcDirective(["${APPS_ORIGIN}"]);
    expect(
      conf.includes(expected),
      `nginx-prod.conf is missing or out of date. It must contain exactly:\n\n${expected}\n`,
    ).toBe(true);
  });

  it("covers each host as BOTH the apex and its subdomains", () => {
    // A single `https://*.youtube.com` would not match `https://youtube.com`, so it would be
    // narrower than the client guard — the browser would block an embed the card accepted.
    const sources = cspFrameSrcDirective();
    for (const d of ALLOWED_IFRAME_HOSTS) {
      expect(sources).toContain(`https://${d} `);
      expect(sources).toContain(`https://*.${d}`);
    }
  });

  it("does not grant 'self' — a same-origin embed is rejected by the card on purpose", () => {
    expect(cspFrameSrcDirective()).not.toContain("'self'");
  });
});


describe("the apps origin in the frame-src", () => {
  const dockerfile = readFileSync(resolve(__dirname, "../../../../../Dockerfile"), "utf8");

  it("is filled in by the nginx image at start, from APPS_* variables only", () => {
    // The config ships as a template, and substitution is restricted to `APPS_*` — otherwise
    // envsubst would also rewrite nginx's own `$host`/`$uri` wherever an env var of that name exists.
    expect(dockerfile).toMatch(/COPY nginx-prod\.conf \/etc\/nginx\/templates\/default\.conf\.template/);
    expect(dockerfile).toMatch(/NGINX_ENVSUBST_FILTER=\^APPS_/);
    // defined but empty by default: an undefined variable would be left as a literal `${APPS_ORIGIN}`
    expect(dockerfile).toMatch(/APPS_ORIGIN=\s*$/m);
  });

  it("is the only `${...}` placeholder in the config, so nothing else can be substituted by accident", () => {
    const conf = readFileSync(NGINX, "utf8");
    const placeholders = [...conf.matchAll(/\$\{[A-Za-z_][A-Za-z0-9_]*\}/g)].map((m) => m[0]);
    expect(new Set(placeholders)).toEqual(new Set(["${APPS_ORIGIN}"]));
  });
});
