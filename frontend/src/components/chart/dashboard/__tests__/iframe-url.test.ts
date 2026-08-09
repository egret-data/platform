import { describe, it, expect } from "vitest";
import {
  IFRAME_REFERRER_POLICY,
  IFRAME_SANDBOX,
  isAllowedIframeHost,
  resolveIframeSrc,
} from "../iframe-url";

describe("resolveIframeSrc — schemes", () => {
  it("accepts an https URL on an allowed host", () => {
    const r = resolveIframeSrc("https://www.youtube.com/embed/dQw4w9WgXcQ");
    expect(r).toStrictEqual({
      ok: true,
      attrs: { src: "https://www.youtube.com/embed/dQw4w9WgXcQ", allow: undefined, allowFullScreen: false },
    });
  });

  it("accepts http as well as https, matching Metabase", () => {
    expect(resolveIframeSrc("http://codepen.io/x/embed/y").ok).toBe(true);
  });

  it("adds https:// to a bare host", () => {
    // Ported from Metabase normalizeUrl (iframe.ts:126-134).
    const r = resolveIframeSrc("figma.com/embed?url=x");
    expect(r.ok && r.attrs.src).toBe("https://figma.com/embed?url=x");
  });

  it("rejects a bare javascript: URL", () => {
    // This one dies in the URL parser, NOT in the scheme check: `javascript:alert(1)` has no
    // `://`, so the normaliser prefixes it and `https://javascript:alert(1)` fails to parse.
    // On its own it would pass against a stubbed scheme guard, which is why the `://` form
    // below exists.
    expect(resolveIframeSrc("javascript:alert('XSS')")).toStrictEqual({
      ok: false, reason: "unparseable", host: null,
    });
  });

  it("rejects a javascript: URL that survives the normaliser", () => {
    // `javascript://youtube.com/%0aalert(1)` DOES match the scheme regex, parses cleanly, and
    // reports hostname "youtube.com" — which is on the allow-list. Only the scheme check kills
    // it. This is the assertion that proves the scheme check exists.
    expect(resolveIframeSrc("javascript://youtube.com/%0aalert(1)")).toStrictEqual({
      ok: false, reason: "unparseable", host: null,
    });
  });

  it("rejects a bare data: URL", () => {
    expect(resolveIframeSrc("data:text/html,<script>alert('XSS')</script>")).toStrictEqual({
      ok: false, reason: "unparseable", host: null,
    });
  });

  it("rejects a data: URL that survives the normaliser", () => {
    // Same shape as the javascript:// case: parses, hostname is allow-listed, scheme is not.
    expect(resolveIframeSrc("data://youtube.com/x")).toStrictEqual({
      ok: false, reason: "unparseable", host: null,
    });
  });

  it("rejects blob:, file: and mailto:", () => {
    // Metabase's isDefaultLinkProtocol permits mailto:; as a frame source it is meaningless.
    expect(resolveIframeSrc("blob://youtube.com/x").ok).toBe(false);
    expect(resolveIframeSrc("file://youtube.com/etc/passwd").ok).toBe(false);
    expect(resolveIframeSrc("mailto://youtube.com").ok).toBe(false);
  });

  it("rejects a same-site relative path", () => {
    expect(resolveIframeSrc("/admin/settings")).toStrictEqual({
      ok: false, reason: "unparseable", host: null,
    });
  });

  it("does not let a relative path's first segment become the hostname", () => {
    // THE assertion for the root-relative guard, and the reason the `/admin/settings` case
    // above cannot stand alone: without the guard THAT one still returns ok:false (it lands on
    // forbidden-host naming "admin"), so it would survive the mutation. This one flips ok
    // false→true, because `https:///youtube.com/x` collapses to host "youtube.com" — which is
    // ALLOW-LISTED. A string that reads as a same-site path would become a live third-party
    // frame. Metabase's normaliser has the same hole.
    expect(resolveIframeSrc("/youtube.com/x")).toStrictEqual({
      ok: false, reason: "unparseable", host: null,
    });
  });

  it("still accepts a protocol-relative URL, which is an absolute reference", () => {
    // The guard rejects `/path`, not `//host/path` — Metabase's normaliser supports the latter
    // deliberately (`replace(/^\/\//, "")`) and it is a real absolute reference.
    expect(resolveIframeSrc("//player.vimeo.com/video/1").ok).toBe(true);
  });

  it("treats empty and whitespace-only input as unset, not as an error", () => {
    expect(resolveIframeSrc("")).toStrictEqual({ ok: false, reason: "empty", host: null });
    expect(resolveIframeSrc("   ")).toStrictEqual({ ok: false, reason: "empty", host: null });
    expect(resolveIframeSrc(undefined)).toStrictEqual({ ok: false, reason: "empty", host: null });
  });
});

describe("resolveIframeSrc — hosts", () => {
  it("rejects a host that is not on the list, and names it", () => {
    expect(resolveIframeSrc("https://example.com/page")).toStrictEqual({
      ok: false, reason: "forbidden-host", host: "example.com",
    });
  });

  it("accepts a subdomain of an allowed host", () => {
    expect(resolveIframeSrc("https://player.vimeo.com/video/1").ok).toBe(true);
  });

  it("does NOT accept a host that merely ends with an allowed one", () => {
    // Metabase's own matcher is `hostname.endsWith(allowedDomain.hostname)` (iframe.ts:235),
    // which accepts this. That is a bug; egret matches on the label boundary.
    expect(isAllowedIframeHost("evilyoutube.com")).toBe(false);
    expect(isAllowedIframeHost("youtube.com.attacker.net")).toBe(false);
    expect(isAllowedIframeHost("youtube.com")).toBe(true);
    expect(isAllowedIframeHost("WWW.YouTube.COM")).toBe(true);
  });

  it("rejects a same-origin src even when its host is allow-listed", () => {
    // allow-same-origin + allow-scripts on a same-origin frame would let the embed reach
    // window.parent and read this SPA's memory. selfOrigin is injectable precisely so this
    // case can be asserted without stubbing window.location.
    expect(resolveIframeSrc("https://www.youtube.com/embed/x", "https://www.youtube.com")).toStrictEqual({
      ok: false, reason: "forbidden-host", host: "www.youtube.com",
    });
  });
});

describe("resolveIframeSrc — embed snippets", () => {
  it("extracts the src from a pasted snippet", () => {
    const r = resolveIframeSrc('<iframe src="https://miro.com/app/live-embed/1"></iframe>');
    expect(r.ok && r.attrs.src).toBe("https://miro.com/app/live-embed/1");
  });

  it("drops an onload handler on the pasted snippet", () => {
    const r = resolveIframeSrc(
      '<iframe src="https://miro.com/x" onload="alert(1)" srcdoc="<b>x</b>"></iframe>',
    );
    expect(r).toStrictEqual({
      ok: true, attrs: { src: "https://miro.com/x", allow: undefined, allowFullScreen: false },
    });
  });

  it("rejects a snippet with no iframe and a snippet with no src", () => {
    expect(resolveIframeSrc("<div>hello</div>")).toStrictEqual({
      ok: false, reason: "unparseable", host: null,
    });
    expect(resolveIframeSrc("<iframe></iframe>")).toStrictEqual({
      ok: false, reason: "unparseable", host: null,
    });
  });

  it("does not treat a string that merely CONTAINS an iframe as a snippet", () => {
    // isIframeSnippet requires the string to START with <iframe, so this goes down the URL
    // path and fails to parse rather than having its src harvested.
    expect(resolveIframeSrc('<script>x</script><iframe src="https://miro.com/x">').ok).toBe(false);
  });

  it("keeps media features from `allow` and drops the dangerous ones", () => {
    const r = resolveIframeSrc(
      '<iframe src="https://www.youtube.com/embed/x" allow="autoplay; encrypted-media; camera; microphone; geolocation; display-capture"></iframe>',
    );
    expect(r.ok && r.attrs.allow).toBe("autoplay; encrypted-media");
  });

  it("strips each feature's origin list so a kept feature is granted to the frame only", () => {
    const r = resolveIframeSrc(
      '<iframe src="https://www.youtube.com/embed/x" allow="autoplay https://evil.example \'self\'"></iframe>',
    );
    expect(r.ok && r.attrs.allow).toBe("autoplay");
  });

  it("omits `allow` entirely when nothing survives the filter", () => {
    const r = resolveIframeSrc('<iframe src="https://www.youtube.com/embed/x" allow="camera; usb"></iframe>');
    expect(r.ok && r.attrs.allow).toBeUndefined();
  });

  it("carries allowfullscreen through as a boolean", () => {
    const r = resolveIframeSrc('<iframe src="https://www.youtube.com/embed/x" allowfullscreen></iframe>');
    expect(r.ok && r.attrs.allowFullScreen).toBe(true);
  });

  it("applies the scheme and host rules to a snippet's src, not only to a bare URL", () => {
    expect(resolveIframeSrc('<iframe src="javascript://youtube.com/%0aalert(1)"></iframe>').ok).toBe(false);
    expect(resolveIframeSrc('<iframe src="https://example.com/x"></iframe>')).toStrictEqual({
      ok: false, reason: "forbidden-host", host: "example.com",
    });
  });
});

describe("resolveIframeSrc — share links become embed links", () => {
  it("rewrites a YouTube watch URL", () => {
    const r = resolveIframeSrc("https://www.youtube.com/watch?v=dQw4w9WgXcQ");
    expect(r.ok && r.attrs.src).toBe("https://www.youtube.com/embed/dQw4w9WgXcQ");
  });

  it("rewrites a youtu.be short link and keeps the playlist", () => {
    const r = resolveIframeSrc("https://youtu.be/dQw4w9WgXcQ?list=PL1");
    expect(r.ok && r.attrs.src).toBe("https://www.youtube.com/embed/dQw4w9WgXcQ?list=PL1");
  });

  it("rewrites a playlist-only URL", () => {
    const r = resolveIframeSrc("https://www.youtube.com/playlist?list=PL1");
    expect(r.ok && r.attrs.src).toBe("https://www.youtube.com/embed/videoseries?list=PL1");
  });

  it("rewrites a Loom share link and a Vimeo link", () => {
    expect(resolveIframeSrc("https://www.loom.com/share/abc123").ok && true).toBe(true);
    const loom = resolveIframeSrc("https://www.loom.com/share/abc123");
    expect(loom.ok && loom.attrs.src).toBe("https://www.loom.com/embed/abc123");
    const vimeo = resolveIframeSrc("https://vimeo.com/123456789");
    expect(vimeo.ok && vimeo.attrs.src).toBe("https://player.vimeo.com/video/123456789");
  });

  it("leaves a host with no transformer alone", () => {
    const r = resolveIframeSrc("https://excalidraw.com/#json=1,2");
    expect(r.ok && r.attrs.src).toBe("https://excalidraw.com/#json=1,2");
  });
});

describe("the frame's own privileges", () => {
  it("grants exactly Metabase's six sandbox tokens", () => {
    // Asserted as a set, not as the joined string, so a reordering does not fail and a silent
    // addition does. The tokens NOT here are the point: no allow-top-navigation, no
    // allow-modals, no allow-downloads.
    expect(IFRAME_SANDBOX.split(" ").sort()).toStrictEqual([
      "allow-forms",
      "allow-popups",
      "allow-popups-to-escape-sandbox",
      "allow-presentation",
      "allow-same-origin",
      "allow-scripts",
    ]);
  });

  it("never grants top navigation, modals or downloads", () => {
    for (const denied of [
      "allow-top-navigation",
      "allow-top-navigation-by-user-activation",
      "allow-modals",
      "allow-downloads",
      "allow-pointer-lock",
      "allow-storage-access-by-user-activation",
    ]) {
      expect(IFRAME_SANDBOX.split(" ")).not.toContain(denied);
    }
  });

  it("uses the referrer policy that leaks only the origin", () => {
    expect(IFRAME_REFERRER_POLICY).toBe("strict-origin-when-cross-origin");
  });
});
