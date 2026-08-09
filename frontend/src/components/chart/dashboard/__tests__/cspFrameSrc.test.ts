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
  it("agree, host for host", () => {
    const conf = readFileSync(NGINX, "utf8");
    const expected = cspFrameSrcDirective();
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
