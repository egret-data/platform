/** URL policy for the iframe dashboard card.
 *
 *  Ported from Metabase's `visualizations/lib/iframe.ts` (`getAllowedIframeAttributes`,
 *  `isAllowedIframeUrl`) and `visualizations/lib/formatting/url.tsx` (`isDefaultLinkProtocol`),
 *  with four deliberate tightenings, each marked at its site.
 *
 *  An iframe card embeds a third party inside an authenticated app, and this module is the only
 *  thing standing between a stored string and a live frame. egret does serve security headers
 *  (`frontend/nginx-prod.conf`, in the SPA `location /`), but they are `X-Frame-Options` and
 *  `X-Content-Type-Options`
 *  — the first governs who may frame EGRET, not what egret may frame. The header that would back
 *  this module up is `Content-Security-Policy: frame-src`, and egret DOES send one: the same
 *  `location /` carries a CSP whose `frame-src` names the embed hosts this module allows. Treat
 *  that as a second lock, not a reason to relax here — the list is maintained in two places and
 *  a browser too old for CSP enforces nothing at all.
 *
 *  So every path here fails CLOSED: anything this module cannot positively prove safe returns
 *  `ok: false`, and the caller renders a stated reason rather than an <iframe>. */

/** Schemes an iframe src may use.
 *
 *  TIGHTENING 1 vs Metabase: `isDefaultLinkProtocol` also permits `mailto:`, which is
 *  meaningless as a frame source. Everything else — `javascript:`, `data:`, `blob:`, `file:`,
 *  `about:` — is rejected here. Note that most hostile inputs never reach this check: they have
 *  no `://`, so `normalizeUrl` prefixes them and the URL parser throws first. The inputs this
 *  check exists for are the ones shaped like `javascript://host/%0apayload`, which parse
 *  cleanly and report an allow-listed hostname. */
const ALLOWED_PROTOCOLS = new Set(["http:", "https:"]);

/** Hosts a card may embed. Metabase ships this list as the `allowed-iframe-hosts` server
 *  setting (`src/metabase/server/settings.clj:12-34`) and lets an admin widen it, including to
 *  `*`. egret has no settings surface for it, so the list is a constant and widening it is a
 *  code change. That is the conservative half of the trade and it is intentional: an unlisted
 *  host cannot be embedded at all, rather than being embeddable by anyone who can reach a
 *  settings page.
 *
 *  Exported from one file so that a future `Content-Security-Policy: frame-src` directive can be
 *  GENERATED from this list rather than drifting from it — which is the property that makes
 *  Metabase's second layer worth having. */
export const ALLOWED_IFRAME_HOSTS = [
  "youtube.com",
  "youtu.be",
  "loom.com",
  "vimeo.com",
  "docs.google.com",
  "calendar.google.com",
  "airtable.com",
  "typeform.com",
  "canva.com",
  "codepen.io",
  "figma.com",
  "grafana.com",
  "miro.com",
  "excalidraw.com",
  "notion.com",
  "atlassian.com",
  "trello.com",
  "asana.com",
  "gist.github.com",
  "linkedin.com",
  "twitter.com",
  "x.com",
] as const;

/**
 * The `frame-src` sources that correspond to `ALLOWED_IFRAME_HOSTS`, so the browser enforces the
 * same list the client guard does.
 *
 * WHY BOTH FORMS PER HOST. The client matcher is `host === d || host.endsWith("." + d)`, i.e. the
 * apex AND its subdomains. CSP's `https://*.youtube.com` does NOT match the bare `youtube.com`,
 * so a single wildcard source would be narrower than the guard and would block an embed the card
 * had already accepted — the two layers must agree or the second one is a bug, not a defence.
 *
 * `'self'` is deliberately absent: `resolveIframeSrc` rejects a same-origin `src` outright,
 * because `allow-same-origin` plus `allow-scripts` on a same-origin frame lets the embed reach
 * `window.parent`. Granting it here would re-open exactly what that rejection closes.
 */
export function frameSrcSources(): string[] {
  return ALLOWED_IFRAME_HOSTS.flatMap((d) => [`https://${d}`, `https://*.${d}`]);
}

/** The full directive, as it must appear in `frontend/nginx-prod.conf`. A test pins the two
 *  together, so adding a host here and forgetting nginx fails the suite rather than silently
 *  leaving the browser layer behind the client one. */
export function cspFrameSrcDirective(extraSources: string[] = []): string {
  return `frame-src ${[...frameSrcSources(), ...extraSources].join(" ")};`;
}

/** Permissions-Policy features an embed may request through its `allow` attribute.
 *
 *  TIGHTENING 2 vs Metabase: `iframe.ts:111-118` copies the pasted snippet's `allow` verbatim,
 *  so one paste can delegate `camera`, `microphone`, `geolocation` or `display-capture` to a
 *  third party. egret keeps only what a media embed needs. */
const ALLOWED_IFRAME_FEATURES = new Set([
  "accelerometer",
  "autoplay",
  "clipboard-write",
  "encrypted-media",
  "fullscreen",
  "gyroscope",
  "picture-in-picture",
  "web-share",
]);

/** Verbatim from Metabase `IFrameViz.tsx:149`. Every token is a capability the frame would not
 *  otherwise have; everything absent is denied.
 *
 *  Granted — `allow-forms` (submit a form), `allow-popups` (`window.open`, `target=_blank`),
 *  `allow-popups-to-escape-sandbox` (those popups are not themselves sandboxed; without it every
 *  popup opens as an opaque-origin dead page), `allow-presentation` (start a presentation
 *  session / cast to a second screen), `allow-same-origin` (the document keeps its OWN origin
 *  instead of a unique opaque one, so the third party's cookies and storage work — it grants
 *  nothing against egret's origin, which is exactly why a same-origin src is refused below),
 *  `allow-scripts` (run JavaScript).
 *
 *  Denied by omission, and the first pair is the reason the attribute is here at all:
 *  `allow-top-navigation` and `allow-top-navigation-by-user-activation` — the embed cannot
 *  navigate the egret tab, so it cannot trade an authenticated session for a phishing page. Also
 *  denied: `allow-modals` (no `alert`/`confirm`/`print` that reads as egret's own chrome),
 *  `allow-downloads`, `allow-pointer-lock`, `allow-orientation-lock`,
 *  `allow-storage-access-by-user-activation`. */
export const IFRAME_SANDBOX =
  "allow-forms allow-popups allow-popups-to-escape-sandbox allow-presentation allow-same-origin allow-scripts";

/** Verbatim from Metabase `IFrameViz.tsx:150`. Requests the frame makes carry only
 *  `https://<egret-host>` as the Referer — never the dashboard path, never its query string,
 *  and nothing at all on an https→http downgrade. */
export const IFRAME_REFERRER_POLICY = "strict-origin-when-cross-origin";

export interface IframeAttributes {
  src: string;
  allow: string | undefined;
  allowFullScreen: boolean;
}

export type IframeResolution =
  | { ok: true; attrs: IframeAttributes }
  /** `empty` = nothing stored yet (a card just added). `unparseable` = not a URL this module
   *  will touch, and the host is deliberately null so a hostile string is never echoed back into
   *  the DOM. `forbidden-host` = a well-formed http(s) URL whose host is not allow-listed; the
   *  host is carried so edit mode can name it. */
  | { ok: false; reason: "empty" | "unparseable" | "forbidden-host"; host: string | null };

const DEFAULT_PROTOCOL = "https://";

/** A ROOT-RELATIVE path (`/admin/settings`) — as distinct from a protocol-relative URL
 *  (`//youtube.com/x`), which is a real absolute reference and stays supported.
 *
 *  TIGHTENING 5 vs Metabase, and this one is a bug in its normaliser rather than a policy
 *  choice. `normalizeUrl` only strips a leading `//`, so a single-slash path is prefixed into
 *  `https:///admin/settings` — and the WHATWG parser collapses the extra slash, making the
 *  path's FIRST SEGMENT the hostname (`host: "admin"`, `pathname: "/settings"`). The consequence
 *  is not cosmetic: `/youtube.com/x` would normalise to `https://youtube.com/x`, pass the
 *  allow-list, and be embedded — a string that looks to a reviewer like a same-site path
 *  silently becoming a third-party frame. A relative path is never a frame source this card
 *  will accept, so it is refused before the normaliser can reinterpret it. */
function isRootRelative(trimmed: string): boolean {
  return trimmed.startsWith("/") && !trimmed.startsWith("//");
}

/** Metabase's normaliser (`iframe.ts:126-134`): a bare host gets `https://`. */
function normalizeUrl(trimmed: string): string {
  if (/^[a-zA-Z][a-zA-Z0-9+.-]*:\/\//.test(trimmed)) return trimmed;
  return DEFAULT_PROTOCOL + trimmed.replace(/^\/\//, "");
}

/** TIGHTENING 3 vs Metabase: its matcher is `hostname.endsWith(allowedDomain.hostname)`
 *  (`iframe.ts:235`), which accepts `evilyoutube.com` for an allowed `youtube.com`. Matching on
 *  the label boundary is the same rule everyone means and nobody writes. */
export function isAllowedIframeHost(hostname: string): boolean {
  const host = hostname.toLowerCase();
  return ALLOWED_IFRAME_HOSTS.some((d) => host === d || host.endsWith("." + d));
}

interface RawAttributes {
  src: string | null;
  allow: string | null;
  allowFullScreen: boolean;
}

/** `<iframe` at position 0, matching Metabase's `isIframeString` (`iframe.ts:136-137`). A string
 *  that merely CONTAINS an iframe is not a snippet — it goes down the URL path and fails. */
function isIframeSnippet(input: string): boolean {
  return input.startsWith("<iframe");
}

/** Read src/allow/allowfullscreen out of a pasted `<iframe …>` snippet.
 *
 *  `DOMParser.parseFromString(html, "text/html")` produces a document with no browsing context:
 *  the parsed <iframe> never loads, an inline `onload=` never fires, and a `<script>` in the
 *  string never runs. That inertness is the whole safety argument, and it is also why exactly
 *  three attributes are read BY NAME — spreading the element's attribute list onto a live
 *  <iframe> would carry `onload` and `srcdoc` across with it. Metabase reads the same three
 *  (`iframe.ts:96-122`). */
function parseSnippet(html: string): RawAttributes | null {
  const el = new DOMParser().parseFromString(html, "text/html").querySelector("iframe");
  if (!el) return null;
  return {
    src: el.getAttribute("src"),
    allow: el.getAttribute("allow"),
    allowFullScreen: el.hasAttribute("allowfullscreen"),
  };
}

/** Keep the media features, drop everything else, and drop each token's origin list so a kept
 *  feature is granted to the frame's own origin (`'src'`, the default) and to nothing else. */
function sanitizeAllow(raw: string | null): string | undefined {
  if (!raw) return undefined;
  const kept = raw
    .split(";")
    .map((token) => token.trim().split(/\s+/)[0].toLowerCase())
    .filter((feature) => ALLOWED_IFRAME_FEATURES.has(feature));
  return kept.length > 0 ? [...new Set(kept)].join("; ") : undefined;
}

/** Share links that a browser refuses to frame, mapped to the host's embed endpoint. Ported
 *  from `iframe.ts:11-63`. Without this, pasting the URL out of a YouTube address bar produces
 *  a card the remote host blocks with X-Frame-Options, which reads as "egret is broken".
 *
 *  Metabase's placeholder-preserving `reconstructUrl` is NOT ported: egret does not substitute
 *  dashboard parameters into an iframe src, so there are no `{{slug}}` placeholders to preserve
 *  and the ids can be encoded properly. */
function toEmbedUrl(url: URL): string {
  const host = url.hostname.toLowerCase().replace(/^www\./, "");
  if (host === "youtube.com" || host === "youtu.be") {
    const videoId =
      host === "youtu.be"
        ? (url.pathname.split("/").filter(Boolean).pop() ?? "")
        : (url.searchParams.get("v") ?? "");
    const listId = url.searchParams.get("list");
    if (videoId) {
      const list = listId ? `?list=${encodeURIComponent(listId)}` : "";
      return `https://www.youtube.com/embed/${encodeURIComponent(videoId)}${list}`;
    }
    if (listId) return `https://www.youtube.com/embed/videoseries?list=${encodeURIComponent(listId)}`;
    return url.toString();
  }
  if (host === "loom.com") {
    return url.origin + url.pathname.replace("/share/", "/embed/") + url.search;
  }
  if (host === "vimeo.com" || host === "player.vimeo.com") {
    const videoId = url.pathname.split("/").filter(Boolean).pop() ?? "";
    return videoId ? `https://player.vimeo.com/video/${encodeURIComponent(videoId)}` : url.toString();
  }
  return url.toString();
}

/**
 * Decide whether a stored card setting may become a live `<iframe src>`.
 *
 * `input` is either a bare URL or a pasted `<iframe …>` snippet — the single free-text setting,
 * matching Metabase's one `iframe` visualization setting.
 *
 * `selfOrigin` is injectable so the same-origin rejection can be asserted without stubbing
 * `window.location`; production callers never pass it.
 */
export function resolveIframeSrc(
  input: string | undefined,
  selfOrigin: string = typeof window === "undefined" ? "" : window.location.origin,
): IframeResolution {
  const trimmed = (input ?? "").trim();
  if (!trimmed) return { ok: false, reason: "empty", host: null };

  const raw: RawAttributes | null = isIframeSnippet(trimmed)
    ? parseSnippet(trimmed)
    : { src: trimmed, allow: null, allowFullScreen: false };
  if (!raw?.src?.trim()) return { ok: false, reason: "unparseable", host: null };

  const src = raw.src.trim();
  if (isRootRelative(src)) return { ok: false, reason: "unparseable", host: null };

  let url: URL;
  try {
    url = new URL(normalizeUrl(src));
  } catch {
    return { ok: false, reason: "unparseable", host: null };
  }

  if (!ALLOWED_PROTOCOLS.has(url.protocol) || !url.hostname) {
    return { ok: false, reason: "unparseable", host: null };
  }

  // TIGHTENING 4 vs Metabase, whose isSafeUrl passes any same-site URL outright
  // (`isSameOrSiteUrlOrigin`). `allow-same-origin` + `allow-scripts` on a SAME-origin frame lets
  // the embed reach `window.parent` and read this SPA's memory; the sandbox above is sized for
  // the cross-origin case, so same-origin is refused rather than sandboxed differently.
  if (selfOrigin && url.origin === selfOrigin) {
    return { ok: false, reason: "forbidden-host", host: url.hostname };
  }

  // Checked before the share→embed rewrite, which only ever maps within the same registrable
  // domain (youtu.be → youtube.com, vimeo.com → player.vimeo.com) and so cannot widen the set.
  if (!isAllowedIframeHost(url.hostname)) {
    return { ok: false, reason: "forbidden-host", host: url.hostname };
  }

  return {
    ok: true,
    attrs: {
      src: toEmbedUrl(url),
      allow: sanitizeAllow(raw.allow),
      allowFullScreen: raw.allowFullScreen,
    },
  };
}
