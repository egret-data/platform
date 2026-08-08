# W9 — iframe Card Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development
> (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.

**Goal:** A dashboard can embed a third-party page — a Loom recording, a Figma frame, a Grafana
panel — as a card, without that page being able to navigate the authenticated egret tab, read the
SPA's memory, or be handed a camera.

**Architecture:** A fourth virtual card type alongside `text` / `heading` / `link`. The whole
workstream is one pure module and one component: `iframe-url.ts` decides whether a stored string
may become an `<iframe src>`, and `IFrameCard.tsx` renders either the frame or a stated reason.
Everything else — the toolbar dropdown, the settings panel branch, the grid size entry, the
`DashboardCardView` dispatch — is four lines each.

**Tech Stack:** React 18, TypeScript, Tailwind v4, shadcn/Radix `DropdownMenu`, vitest + Testing
Library. No new dependency: the snippet parser is the platform's own `DOMParser`.

**Design reference:** `docs/en/specs/2026-08-09-dashboard-parity-design.md` §4 W9. Note §5 — the
**action button card is out of scope** because egret has no writeback subsystem. Nothing in this
plan touches it.

**Branch:** `feat/dashboard-parity-09-iframe-card`, from `main`.

---

## Background the implementer needs

### The spec's "frontend-only" claim is true — here is the verification

The design says "egret's card-type column is free text with no CHECK
(`migrations/0015_dashboard.sql:21`), so this is frontend-only." Checked, four ways:

- `migrations/0015_dashboard.sql:21` — `card_type text NOT NULL DEFAULT 'chart'`. No CHECK, no
  enum type, no trigger. `options` at `:25` is `jsonb NOT NULL DEFAULT '{}'`, so a new
  `options.iframe` key needs no migration either.
- `crates/egret-analytics-api/src/bi.rs:387-388` — `CardInput.card_type` is a bare `String` with
  `#[serde(default = "default_card_type")]`. `DashboardCardDto.card_type` at `:344` is also a bare
  `String`. No `serde(rename_all)`, no custom deserializer, no enum.
- `crates/egret-bi/src/routes.rs:2705` validates exactly one field on the dashboard PUT
  (`width != "fixed" && b.width != "full"`), and `require_cards_in_ws` at `:2547-2566` checks only
  that a card's `chart_id` belongs to the workspace — a card with `chart_id: None` skips the loop
  body entirely.
- Every other `card_type` occurrence in `crates/` is plumbing (`egret-bi/src/dashboard.rs:36,80,93,
  368,371,443,446`, `egret-bi/src/model.rs:191,263,305`) or a test fixture. There is no `match` on
  it anywhere in Rust.

An unknown `card_type` therefore round-trips through PUT → Postgres → GET unchanged. **No backend
change, no migration, no `cargo` gate in this workstream.** (One small correction to the design:
it cites the width validation as `routes.rs:2702-2706` without a crate; the file is
`crates/egret-bi/src/routes.rs`, not `crates/egret-analytics-api/`, which has no `routes.rs`.)

### egret ships security headers, but none of them constrains what egret may frame

Be precise about this, because the rationale for the next four tasks rests on it and an
overstated claim is one a reviewer can discount.

egret **does** serve response security headers. `frontend/nginx-prod.conf:58-59`, on the SPA
`location /` block, sets:

```nginx
        add_header X-Frame-Options "SAMEORIGIN" always;
        add_header X-Content-Type-Options "nosniff" always;
```

Neither one bears on this workstream, and `X-Frame-Options` in particular points the wrong way.
It governs **who may frame egret** — it stops a third-party page embedding the dashboard in an
iframe of its own. It says nothing whatsoever about **what egret may frame**. The header that
constrains outbound framing is `Content-Security-Policy: frame-src`, and there is genuinely no
`Content-Security-Policy` anywhere in this repository — verified across `crates/`, `frontend/`
and `deploy/`, and across every nginx config:

```bash
/usr/bin/grep -rn "Content-Security-Policy" crates frontend/src deploy
/usr/bin/grep -rn "add_header" --include="*.conf" .
```

The first returns nothing. The second returns only the four `add_header` lines in
`frontend/nginx-prod.conf` (`Cache-Control`, `Content-Type`, and the two above) — no CSP among
them.

So the asymmetry with Metabase is real and it is specifically about `frame-src`: Metabase derives
its `frame-src` directive from the *same* `allowed-iframe-hosts` setting the client-side check
reads (`src/metabase/server/middleware/security.clj:224`), giving it two independent layers that
fail independently. egret has one. If `iframe-url.ts` lets a URL through, the browser loads it.

That is the reason this plan spends four tasks on a card that renders one element:
`iframe-url.ts` is a sole control, not a defence-in-depth nicety. Treat every early return in it
as load-bearing.

(If a future workstream adds a CSP, `ALLOWED_IFRAME_HOSTS` is exported from one file precisely so
the `frame-src` directive can be generated from it rather than drifting from it — which is the
property that makes Metabase's second layer worth having.)

### The policy being ported, decision by decision

Read `metabase/frontend/src/metabase/visualizations/lib/iframe.ts` alongside this. Egret's port
matches it except where marked.

| Question | Metabase | egret | Why |
|---|---|---|---|
| Schemes accepted | `http:`, `https:`, `mailto:` (`isDefaultLinkProtocol`, `formatting/url.tsx:23-27`), plus a blanket pass for any same-site URL (`isSafeUrl` → `isSameOrSiteUrlOrigin`) | `http:`, `https:` only | `mailto:` is meaningless as a frame source, and the same-site pass is actively harmful here (next row) |
| Same-origin src | passes `isSafeUrl`; only the host list stops it | **rejected outright** | `allow-same-origin` + `allow-scripts` on a *same-origin* frame lets the embed reach `window.parent`. Cross-origin is the case the sandbox is sized for |
| Host allow-list | admin setting `allowed-iframe-hosts`, 22 hosts by default (`src/metabase/server/settings.clj:12-34`), `*` = anything | the same 22 hosts as a **constant** | egret has no admin settings surface for it. Widening is a code change — deliberately the conservative half of the trade |
| Host matching | `hostname.endsWith(allowed)` (`iframe.ts:235`) | `host === d \|\| host.endsWith("." + d)` | Metabase's predicate accepts `evilyoutube.com` for an allowed `youtube.com`. That is a bug, not a policy; do not port it |
| `<iframe …>` snippet accepted? | **yes** — `DOMParser` on `text/html`, then `src` / `allow` / `allowfullscreen` read by name (`iframe.ts:96-122`) | same | A `DOMParser` document has no browsing context: the parsed frame never loads, `onload=` never fires, `<script>` never runs. That inertness is the entire safety argument |
| `allow` attribute | copied from the snippet **verbatim** | filtered to eight media features, origin lists stripped | Verbatim copy lets one paste delegate `camera`, `microphone`, `geolocation`, `display-capture` to a third party |
| `sandbox` | `allow-forms allow-popups allow-popups-to-escape-sandbox allow-presentation allow-same-origin allow-scripts` | identical, verbatim | see Task 1's comment for what each token buys and what the omissions deny |
| `referrerpolicy` | `strict-origin-when-cross-origin` | identical | the embed learns `https://<egret-host>`, never the dashboard path or its query string |
| Bad stored URL | edit mode names the domain; view mode shows a generic message (`IFrameViz.tsx:134-139`) | identical | a viewer who cannot fix the card gains nothing from the hostname, and dashboards are shareable |

### Deliberate non-goals for this workstream

- **No parameter substitution into the src.** Metabase interpolates `{{slug}}` into the iframe URL
  with `urlEncode: true` (`IFrameViz.tsx:77-87`). egret's own `interpolateParams`
  (`params.ts:351-362`) does **not** encode — it is written for markdown bodies. Feeding it a URL
  would let a filter value carrying `&`, `#` or `../` rewrite the request. Wiring substitution
  safely means a second, encoding interpolator; that is its own change with its own tests, not a
  rider on this one. The settings panel says nothing about `{{slug}}`, so no user is promised it.
- **No auto-height.** An iframe's content height is cross-origin and unmeasurable from the parent,
  so `AutoHeightMeasure` would read the container it already sized. Task 4 withholds the toggle.
- **No admin allowed-hosts setting.** The constant is exported from one file so adding a setting
  later is a one-call change.

---

## File structure

| Path | Responsibility after this workstream |
|---|---|
| `frontend/src/components/chart/dashboard/iframe-url.ts` | **New.** The whole URL policy: scheme check, host allow-list, same-origin rejection, snippet parsing, `allow` filtering, share→embed rewriting, and the `sandbox` / `referrerpolicy` constants. Pure — no React, no DOM beyond `DOMParser`. |
| `frontend/src/components/chart/dashboard/IFrameCard.tsx` | **New.** Renders the frame, or the fail-closed reason. Holds no policy of its own. |
| `frontend/src/components/chart/dashboard-grid.ts` | Gains the `iframe` entry in `CARD_SIZES_BY_VIZ`. |
| `frontend/src/components/chart/dashboard/DashboardCardView.tsx` | Gains one `card_type === "iframe"` branch. |
| `frontend/src/components/chart/dashboard/CardSettingsPanel.tsx` | Gains the iframe settings branch. |
| `frontend/src/components/chart/dashboard/DashboardEditToolbar.tsx` | The link button becomes a "Add a link or iframe" dropdown. |
| `frontend/src/routes/explore/DashboardView.tsx` | Wiring only: `addVirtualCard` accepts `"iframe"`, the toolbar gets `onAddIframe`. |

The plan index lists a single new file, `IFrameCard.tsx`, "iframe card + URL allow-list". This
plan splits it in two. The reason is Task 1 Step 5: the guard's mutation proof has to run the
policy without rendering React, and a policy that can only be exercised through a component is a
policy that gets tested through display strings — the exact failure the chart programme hit six
times.

---

## Task 1: The URL policy

**Files:**
- Create: `frontend/src/components/chart/dashboard/iframe-url.ts`
- Test: `frontend/src/components/chart/dashboard/__tests__/iframe-url.test.ts`

- [ ] **Step 1: Write the failing test**

Create `frontend/src/components/chart/dashboard/__tests__/iframe-url.test.ts`:

```ts
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
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd frontend && npx vitest run src/components/chart/dashboard/__tests__/iframe-url.test.ts`

Expected: FAIL — `Failed to resolve import "../iframe-url"`.

- [ ] **Step 3: Write the implementation**

Create `frontend/src/components/chart/dashboard/iframe-url.ts`:

```ts
/** URL policy for the iframe dashboard card.
 *
 *  Ported from Metabase's `visualizations/lib/iframe.ts` (`getAllowedIframeAttributes`,
 *  `isAllowedIframeUrl`) and `visualizations/lib/formatting/url.tsx` (`isDefaultLinkProtocol`),
 *  with four deliberate tightenings, each marked at its site.
 *
 *  An iframe card embeds a third party inside an authenticated app, and this module is the only
 *  thing standing between a stored string and a live frame. egret does serve security headers
 *  (`frontend/nginx-prod.conf:58-59`), but they are `X-Frame-Options` and `X-Content-Type-Options`
 *  — the first governs who may frame EGRET, not what egret may frame. The header that would back
 *  this module up is `Content-Security-Policy: frame-src`, and egret sends no CSP at all.
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
 *  settings page. */
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

  let url: URL;
  try {
    url = new URL(normalizeUrl(raw.src.trim()));
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
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cd frontend && npx vitest run src/components/chart/dashboard/__tests__/iframe-url.test.ts`

Expected: PASS, 31 tests.

- [ ] **Step 5: Prove the tests survive mutation**

Four separate mutations. Run the file after each, restore before the next. Score by **process
exit code**, never by reading a `PASS (n)` line — that line is the rtk wrapper's reformatting and
raw vitest never prints it.

1. **Remove the scheme guard.** Delete the `!ALLOWED_PROTOCOLS.has(url.protocol) ||` clause,
   leaving `if (!url.hostname)`. Re-run.
   Expected: FAIL on exactly four assertions — "rejects a javascript: URL that survives the
   normaliser", "rejects a data: URL that survives the normaliser", "rejects blob:, file: and
   mailto:", and "applies the scheme and host rules to a snippet's src". The two **bare**-colon
   tests must still PASS, because they die in the URL parser and never reach this line. If the
   bare ones fail too, something other than the scheme check is doing the rejecting and the
   scheme check is not proven at all.
2. **Widen the host matcher.** Change `isAllowedIframeHost` to
   `return ALLOWED_IFRAME_HOSTS.some((d) => host.endsWith(d));` — Metabase's own predicate.
   Re-run. Expected: FAIL on `evilyoutube.com`.
3. **Drop the same-origin rejection.** Delete the `selfOrigin && url.origin === selfOrigin`
   block. Re-run. Expected: FAIL on "rejects a same-origin src even when its host is
   allow-listed".
4. **Pass `allow` through verbatim.** Change `sanitizeAllow` to `return raw ?? undefined;`.
   Re-run. Expected: FAIL on the three `allow` filtering tests.

This step is not optional. Six assertions in the chart programme passed against a stubbed
implementation, and the only thing that caught them was doing exactly this.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/components/chart/dashboard/iframe-url.ts \
        frontend/src/components/chart/dashboard/__tests__/iframe-url.test.ts
git commit -m "feat(dashboard): URL policy for iframe cards — https-only, host allow-list, sandboxed"
```

---

## Task 2: The card renders the frame, or says why it will not

**Files:**
- Create: `frontend/src/components/chart/dashboard/IFrameCard.tsx`
- Test: `frontend/src/components/chart/dashboard/__tests__/IFrameCard.test.tsx`

- [ ] **Step 1: Write the failing test**

Create `frontend/src/components/chart/dashboard/__tests__/IFrameCard.test.tsx`:

```tsx
import { describe, it, expect } from "vitest";
import { render, screen } from "@testing-library/react";
import { IFrameCard } from "../IFrameCard";
import { IFRAME_SANDBOX } from "../iframe-url";

describe("IFrameCard (allowed content)", () => {
  it("renders the frame with the sandbox, referrer policy and src", () => {
    const { container } = render(<IFrameCard iframe="https://www.loom.com/share/abc123" />);
    const frame = container.querySelector("iframe");
    expect(frame).not.toBeNull();
    // Compare against the exported constant, NOT a literal: the constant's own contents are
    // pinned token by token in iframe-url.test.ts, and duplicating the string here would let a
    // change pass by being made in two places at once.
    expect(frame).toHaveAttribute("sandbox", IFRAME_SANDBOX);
    expect(frame).toHaveAttribute("referrerpolicy", "strict-origin-when-cross-origin");
    expect(frame).toHaveAttribute("src", "https://www.loom.com/embed/abc123");
  });

  it("sets no allow attribute when the input asked for none", () => {
    const { container } = render(<IFrameCard iframe="https://www.loom.com/share/abc123" />);
    expect(container.querySelector("iframe")).not.toHaveAttribute("allow");
  });

  it("carries the filtered allow list through from a snippet", () => {
    const { container } = render(
      <IFrameCard iframe='<iframe src="https://www.youtube.com/embed/x" allow="autoplay; camera" allowfullscreen></iframe>' />,
    );
    const frame = container.querySelector("iframe");
    expect(frame).toHaveAttribute("allow", "autoplay");
    expect(frame).toHaveAttribute("allowfullscreen");
  });

  it("gives the frame an accessible name", () => {
    // WCAG 4.1.2 / technique H64: a frame with no title is unlabelled to a screen reader.
    render(<IFrameCard iframe="https://www.loom.com/share/abc123" />);
    expect(screen.getByTitle("Embedded content")).toBeInTheDocument();
  });

  it("does not swallow the click that opens the card settings while editing", () => {
    // A live iframe eats pointer events, so in edit mode the click would never reach the card
    // wrapper that opens the settings panel (DashboardView.tsx:628).
    const { container } = render(<IFrameCard iframe="https://www.loom.com/share/abc123" editing />);
    expect(container.querySelector("iframe")).toHaveClass("pointer-events-none");
  });
});

describe("IFrameCard (fails closed)", () => {
  it("renders no iframe at all for an unset card", () => {
    const { container } = render(<IFrameCard iframe="" editing />);
    expect(container.querySelector("iframe")).toBeNull();
    expect(screen.getByText("No embed set")).toBeInTheDocument();
  });

  it("renders no iframe for a javascript: URL, and says something is wrong", () => {
    const { container } = render(<IFrameCard iframe="javascript://youtube.com/%0aalert(1)" editing />);
    expect(container.querySelector("iframe")).toBeNull();
    expect(screen.getByText("There was a problem rendering this content.")).toBeInTheDocument();
  });

  it("renders no iframe for a data: URL", () => {
    const { container } = render(<IFrameCard iframe="data://youtube.com/x" editing />);
    expect(container.querySelector("iframe")).toBeNull();
  });

  it("names the blocked host while editing", () => {
    const { container } = render(<IFrameCard iframe="https://example.com/x" editing />);
    expect(container.querySelector("iframe")).toBeNull();
    expect(screen.getByText("example.com can't be embedded in an iframe card.")).toBeInTheDocument();
  });

  it("does NOT name the blocked host in view mode", () => {
    // Metabase draws the same line (IFrameViz.tsx:134-139): a viewer who cannot change the card
    // gains nothing from the hostname, and a dashboard is shareable.
    const { container } = render(<IFrameCard iframe="https://example.com/x" />);
    expect(container.querySelector("iframe")).toBeNull();
    expect(screen.queryByText(/example\.com/)).toBeNull();
    expect(screen.getByText("There was a problem rendering this content.")).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd frontend && npx vitest run src/components/chart/dashboard/__tests__/IFrameCard.test.tsx`

Expected: FAIL — `Failed to resolve import "../IFrameCard"`.

- [ ] **Step 3: Write the implementation**

Create `frontend/src/components/chart/dashboard/IFrameCard.tsx`:

```tsx
import { Lock } from "lucide-react";
import { IFRAME_REFERRER_POLICY, IFRAME_SANDBOX, resolveIframeSrc } from "./iframe-url";

/** A dashboard iframe card: third-party content embedded inside an authenticated app.
 *
 *  Every render path goes through `resolveIframeSrc`, which fails closed — a setting this
 *  component cannot prove safe renders a stated reason, never an empty box and never an
 *  <iframe> with an unchecked src. The card holds no policy of its own; if you find yourself
 *  adding a URL check here, it belongs in `iframe-url.ts` where it is testable without React. */
export function IFrameCard({ iframe, editing }: { iframe?: string; editing?: boolean }) {
  const resolved = resolveIframeSrc(iframe);

  if (!resolved.ok) {
    if (resolved.reason === "empty") {
      return (
        <div className="flex h-full items-center justify-center p-2 text-sm text-muted-foreground">
          No embed set
        </div>
      );
    }
    // The host is named only while editing, matching Metabase's split between
    // ForbiddenDomainError and GenericError (IFrameViz.tsx:134-139).
    const namesHost = editing && resolved.reason === "forbidden-host" && !!resolved.host;
    return (
      <div className="flex h-full flex-col items-center justify-center gap-1 p-3 text-center">
        <Lock className="h-4 w-4 text-muted-foreground" aria-hidden />
        <p className="text-sm text-foreground">
          {namesHost
            ? `${resolved.host} can't be embedded in an iframe card.`
            : "There was a problem rendering this content."}
        </p>
        {editing && (
          <p className="text-xs text-muted-foreground">
            Only https pages on egret's allowed hosts can be embedded.
          </p>
        )}
      </div>
    );
  }

  return (
    <iframe
      title="Embedded content"
      // A live iframe swallows pointer events, so while editing it would eat the click that
      // opens the card's settings panel and the drag that moves the card.
      className={`h-full w-full border-0 ${editing ? "pointer-events-none" : ""}`}
      sandbox={IFRAME_SANDBOX}
      referrerPolicy={IFRAME_REFERRER_POLICY}
      {...(resolved.attrs.allow ? { allow: resolved.attrs.allow } : {})}
      allowFullScreen={resolved.attrs.allowFullScreen}
      src={resolved.attrs.src}
    />
  );
}
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cd frontend && npx vitest run src/components/chart/dashboard/__tests__/IFrameCard.test.tsx`

Expected: PASS, 10 tests.

- [ ] **Step 5: Prove the tests survive mutation**

Replace the whole `if (!resolved.ok)` block with `if (!resolved.ok) return <div />;` and re-run.

Expected: FAIL on all six "fails closed" tests. Note which ones do **not** fail: none should
still pass, because each of the six asserts a specific string or a specific absence. If any
passes against the stub, that assertion was reading something the stub also produces — rewrite
it before restoring.

Then restore, and mutate the other direction: drop `sandbox={IFRAME_SANDBOX}` from the element
and re-run. Expected: FAIL on the first test. Restore.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/components/chart/dashboard/IFrameCard.tsx \
        frontend/src/components/chart/dashboard/__tests__/IFrameCard.test.tsx
git commit -m "feat(dashboard): iframe card renders a sandboxed frame or a stated refusal"
```

---

## Task 3: The grid knows how big an iframe card is

**Files:**
- Modify: `frontend/src/components/chart/dashboard-grid.ts`
- Test: `frontend/src/components/chart/__tests__/dashboard-grid.test.ts` (existing file)

- [ ] **Step 1: Write the failing test**

Append to the `describe("sizeForViz", …)` block in
`frontend/src/components/chart/__tests__/dashboard-grid.test.ts`:

```ts
  it("sizes an iframe card 12x8, min 4x3", () => {
    // Metabase dashboards/constants.cljc:26 — :iframe {:min {:width 4 :height 3}
    // :default {:width 12 :height 8}}. Asserted as the whole object so a change to either
    // dimension is caught; the generic-chart fallback is 12x6/4x3, so a missing entry would
    // pass a `min`-only assertion.
    expect(sizeForViz("iframe")).toEqual({ default: { w: 12, h: 8 }, min: { w: 4, h: 3 } });
  });
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd frontend && npx vitest run src/components/chart/__tests__/dashboard-grid.test.ts`

Expected: FAIL — received `{ default: { w: 12, h: 6 }, min: { w: 4, h: 3 } }`, the
`GENERIC_CHART` fallback. That the *height* is the only difference is exactly why the assertion
compares the whole object.

- [ ] **Step 3: Write the implementation**

In `frontend/src/components/chart/dashboard-grid.ts`, add one row to `CARD_SIZES_BY_VIZ`
immediately after the `link` entry (line 77):

```ts
  iframe:      { default: { w: 12, h: 8 }, min: { w: 4, h: 3 } },
```

- [ ] **Step 4: Run the test to verify it passes**

Run: `cd frontend && npx vitest run src/components/chart/__tests__/dashboard-grid.test.ts`

Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add frontend/src/components/chart/dashboard-grid.ts \
        frontend/src/components/chart/__tests__/dashboard-grid.test.ts
git commit -m "feat(dashboard): iframe card grid size — default 12x8, min 4x3"
```

---

## Task 4: The card type is dispatched and configurable

**Files:**
- Modify: `frontend/src/components/chart/dashboard/DashboardCardView.tsx`
- Modify: `frontend/src/components/chart/dashboard/CardSettingsPanel.tsx`
- Test: `frontend/src/components/chart/dashboard/__tests__/DashboardCardView.test.tsx` (existing)
- Test: `frontend/src/components/chart/dashboard/__tests__/CardSettingsPanel.test.tsx` (existing)

- [ ] **Step 1: Write the failing tests**

Append to `frontend/src/components/chart/dashboard/__tests__/DashboardCardView.test.tsx`, inside
the existing `describe("DashboardCardView", …)` block, next to the link-card test:

```tsx
  it("routes an iframe card and does NOT fetch data", () => {
    const { container } = render(
      <DashboardCardView ws="eng" dashboardId="d1" chart={undefined}
        card={card({ card_type: "iframe", options: { iframe: "https://www.loom.com/share/abc" } })} />,
    );
    expect(container.querySelector("iframe")).toHaveAttribute("src", "https://www.loom.com/embed/abc");
    expect(useDashboardCardData).not.toHaveBeenCalled();
  });
  it("routes an iframe card with a forbidden host to the refusal, still without fetching", () => {
    const { container } = render(
      <DashboardCardView ws="eng" dashboardId="d1" chart={undefined}
        card={card({ card_type: "iframe", options: { iframe: "https://example.com" } })} />,
    );
    expect(container.querySelector("iframe")).toBeNull();
    expect(useDashboardCardData).not.toHaveBeenCalled();
  });
```

Append a new `describe` block to
`frontend/src/components/chart/dashboard/__tests__/CardSettingsPanel.test.tsx`:

```tsx
describe("CardSettingsPanel (iframe card)", () => {
  const iframeCard = (opts: Record<string, unknown> = {}): DraftCard => ({
    id: "if1", chart_id: null, card_type: "iframe", grid_col: 0, grid_row: 0,
    size_x: 12, size_y: 8, options: opts, tab_id: null, tabKey: null,
  });

  it("edits the single free-text embed setting and writes only that key", () => {
    const onUpdateOptions = vi.fn();
    render(<CardSettingsPanel card={iframeCard()} chart={undefined} parameters={[param]} tabs={[]} onUpdateOptions={onUpdateOptions} onMoveToTab={vi.fn()} />);
    fireEvent.change(screen.getByLabelText("iframe url or embed code"), {
      target: { value: "https://www.loom.com/share/abc" },
    });
    // toStrictEqual, not toEqual: the point is that the patch carries NOTHING else.
    expect(onUpdateOptions.mock.calls[0][0]).toStrictEqual({ iframe: "https://www.loom.com/share/abc" });
  });

  it("shows the stored value", () => {
    render(<CardSettingsPanel card={iframeCard({ iframe: "https://figma.com/embed?x=1" })} chart={undefined} parameters={[]} tabs={[]} onUpdateOptions={vi.fn()} onMoveToTab={vi.fn()} />);
    expect(screen.getByLabelText("iframe url or embed code")).toHaveValue("https://figma.com/embed?x=1");
  });

  it("offers no auto-height toggle, because a cross-origin frame cannot be measured", () => {
    render(<CardSettingsPanel card={iframeCard()} chart={undefined} parameters={[]} tabs={[]} onUpdateOptions={vi.fn()} onMoveToTab={vi.fn()} />);
    expect(screen.queryByLabelText("auto height")).toBeNull();
  });

  it("offers no hide-when-empty toggle, because an iframe card runs no query", () => {
    render(<CardSettingsPanel card={iframeCard()} chart={undefined} parameters={[]} tabs={[]} onUpdateOptions={vi.fn()} onMoveToTab={vi.fn()} />);
    expect(screen.queryByLabelText("hide when empty")).toBeNull();
  });

  it("does NOT fall through to the chart-card panel", () => {
    // Without its own branch, an iframe card lands on the default return at the bottom of the
    // file and gets a Title field and an On-click select, neither of which applies.
    render(<CardSettingsPanel card={iframeCard()} chart={undefined} parameters={[]} tabs={[]} onUpdateOptions={vi.fn()} onMoveToTab={vi.fn()} />);
    expect(screen.queryByLabelText("on click")).toBeNull();
  });
});
```

- [ ] **Step 2: Run the tests to verify they fail**

Run each as its own command.

```bash
cd frontend && npx vitest run src/components/chart/dashboard/__tests__/DashboardCardView.test.tsx
```

Expected: FAIL — the iframe card falls through to `ChartCardTileView`, which renders "Chart not
found" and there is no `<iframe>` in the container.

```bash
cd frontend && npx vitest run src/components/chart/dashboard/__tests__/CardSettingsPanel.test.tsx
```

Expected: FAIL — no element labelled `iframe url or embed code`; the "does NOT fall through"
test fails too, because the on-click select is present.

- [ ] **Step 3: Write the implementation**

In `frontend/src/components/chart/dashboard/DashboardCardView.tsx`, add the import next to the
other card imports (after line 13):

```tsx
import { IFrameCard } from "./IFrameCard";
```

and the dispatch branch immediately after the `link` branch (line 107):

```tsx
  // Not wrapped in AutoHeightMeasure: a cross-origin frame's content height is unreadable from
  // the parent, so the measurement would only report back the height the grid already set.
  if (card.card_type === "iframe") return <IFrameCard iframe={asString(card.options.iframe)} editing={editing} />;
```

In `frontend/src/components/chart/dashboard/CardSettingsPanel.tsx`, add the branch immediately
after the `link` branch (after line 76):

```tsx
  if (card.card_type === "iframe") {
    return (
      <div className="flex flex-col gap-2 p-3">
        <div className="text-sm font-medium">Iframe</div>
        <textarea
          aria-label="iframe url or embed code"
          rows={4}
          placeholder={'https://…  or  <iframe src="https://…" />'}
          className="w-full resize-y rounded border bg-background px-2 py-1 text-sm"
          value={(card.options.iframe as string) ?? ""}
          onChange={(e) => onUpdateOptions({ iframe: e.target.value })}
        />
        <p className="text-xs text-muted-foreground">
          Only https pages on egret's allowed hosts can be embedded. Anything else shows a
          refusal on the card instead of the page.
        </p>
        {/* No auto-height row: a cross-origin frame's content height cannot be measured from
            here, so the toggle would promise something it cannot do. */}
        {moveToTab}
      </div>
    );
  }
```

- [ ] **Step 4: Run the tests to verify they pass**

```bash
cd frontend && npx vitest run src/components/chart/dashboard/__tests__/DashboardCardView.test.tsx
```

Expected: PASS, 12 tests (10 existing + 2).

```bash
cd frontend && npx vitest run src/components/chart/dashboard/__tests__/CardSettingsPanel.test.tsx
```

Expected: PASS, 13 tests (8 existing + 5).

- [ ] **Step 5: Prove the tests survive mutation**

Delete the `if (card.card_type === "iframe")` branch from `CardSettingsPanel.tsx` and re-run its
test. Expected: FAIL on all five, including "does NOT fall through" — that last one is the reason
this mutation is worth doing, because the other four would also fail if the branch merely
rendered nothing.

Delete the dispatch branch from `DashboardCardView.tsx` and re-run its test. Expected: FAIL on
both new tests. Restore both.

- [ ] **Step 6: Commit**

```bash
git add frontend/src/components/chart/dashboard/DashboardCardView.tsx \
        frontend/src/components/chart/dashboard/CardSettingsPanel.tsx \
        frontend/src/components/chart/dashboard/__tests__/DashboardCardView.test.tsx \
        frontend/src/components/chart/dashboard/__tests__/CardSettingsPanel.test.tsx
git commit -m "feat(dashboard): dispatch and configure the iframe card type"
```

---

## Task 5: "Add a link or iframe" — the toolbar dropdown

**Files:**
- Modify: `frontend/src/components/chart/dashboard/DashboardEditToolbar.tsx`
- Modify: `frontend/src/routes/explore/DashboardView.tsx:288-302, 510-518`
- Test: `frontend/src/components/chart/dashboard/__tests__/DashboardEditToolbar.test.tsx` (new)

egret's link control is a plain button (`DashboardEditToolbar.tsx:70-72`). Metabase's is a menu
labelled "Add a link or iframe" with two items, Link and Iframe
(`DashboardHeader/buttons/AddLinkOrEmbedButton.tsx:39-62`). The heading/text control in this same
toolbar is already a `DropdownMenu` (`:57-68`) — copy its shape, including
`onCloseAutoFocus={(e) => e.preventDefault()}`, which is there because Radix's close teardown
blurs focus to `<body>` after the new card mounts (see the comment in `HeadingCard.tsx:7-8`).

- [ ] **Step 1: Write the failing test**

Create `frontend/src/components/chart/dashboard/__tests__/DashboardEditToolbar.test.tsx`:

```tsx
import { describe, it, expect, vi } from "vitest";
import { render, screen, fireEvent } from "@testing-library/react";
import { DashboardEditToolbar } from "../DashboardEditToolbar";

function setup() {
  const props = {
    onAddChart: vi.fn(), onAddText: vi.fn(), onAddHeading: vi.fn(),
    onAddLink: vi.fn(), onAddIframe: vi.fn(), onAddSection: vi.fn(), onAddFilter: vi.fn(),
  };
  render(<DashboardEditToolbar {...props} />);
  return props;
}

describe("DashboardEditToolbar link/iframe menu", () => {
  it("uses Metabase's label for the trigger", () => {
    // Verbatim from AddLinkOrEmbedButton.tsx:43-46, because this toolbar is measured against it.
    setup();
    expect(screen.getByRole("button", { name: "add a link or iframe" })).toBeInTheDocument();
  });

  it("offers Link and Iframe, in that order", () => {
    setup();
    fireEvent.click(screen.getByRole("button", { name: "add a link or iframe" }));
    const items = screen.getAllByRole("menuitem").map((el) => el.textContent);
    expect(items).toStrictEqual(["Link", "Iframe"]);
  });

  it("adds a link from the menu, not from a bare button", () => {
    // The regression this guards: turning the button into a menu without moving its handler
    // leaves a menu whose Link item does nothing.
    const props = setup();
    fireEvent.click(screen.getByRole("button", { name: "add a link or iframe" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Link" }));
    expect(props.onAddLink).toHaveBeenCalledTimes(1);
    expect(props.onAddIframe).not.toHaveBeenCalled();
  });

  it("adds an iframe from the menu", () => {
    const props = setup();
    fireEvent.click(screen.getByRole("button", { name: "add a link or iframe" }));
    fireEvent.click(screen.getByRole("menuitem", { name: "Iframe" }));
    expect(props.onAddIframe).toHaveBeenCalledTimes(1);
    expect(props.onAddLink).not.toHaveBeenCalled();
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cd frontend && npx vitest run src/components/chart/dashboard/__tests__/DashboardEditToolbar.test.tsx`

Expected: FAIL — `DashboardEditToolbar` has no `onAddIframe` prop, and the accessible name is
still `add link`.

If the "offers Link and Iframe" test errors rather than failing, the Radix menu did not open
under `fireEvent.click`. The heading menu in this same file opens fine under it in
`AddPanel.test.tsx`'s sibling specs; if it does not here, switch that one interaction to
`userEvent.click` (already a dependency) rather than reaching for `act`.

- [ ] **Step 3: Write the implementation**

In `frontend/src/components/chart/dashboard/DashboardEditToolbar.tsx`, add `onAddIframe` to
`Props` (after `onAddLink` on line 14):

```tsx
  onAddIframe: () => void;
```

Add it to the destructured signature on line 50:

```tsx
export function DashboardEditToolbar({ onAddChart, onAddText, onAddHeading, onAddLink, onAddIframe, onAddSection, onAddFilter }: Props) {
```

Replace the plain link button (lines 70-72) with a dropdown of the same shape as the
heading/text one above it:

```tsx
      {/* Metabase pairs these two behind one trigger, labelled "Add a link or iframe"
          (AddLinkOrEmbedButton.tsx:39-62). onCloseAutoFocus is prevented for the same reason as
          the heading menu above: Radix's teardown blurs focus to <body> after the card mounts. */}
      <DropdownMenu>
        <DropdownMenuTrigger asChild>
          <button type="button" className={ICON} aria-label="add a link or iframe" title="Add a link or iframe">
            <Link2 className="h-4 w-4" />
            <ChevronDown className="h-3 w-3" />
          </button>
        </DropdownMenuTrigger>
        <DropdownMenuContent align="end" onCloseAutoFocus={(e) => e.preventDefault()}>
          <DropdownMenuItem onClick={onAddLink}>Link</DropdownMenuItem>
          <DropdownMenuItem onClick={onAddIframe}>Iframe</DropdownMenuItem>
        </DropdownMenuContent>
      </DropdownMenu>
```

The `Link2` and `ChevronDown` imports on line 1 are already present; nothing changes there.

- [ ] **Step 4: Run the test to verify it passes**

Run: `cd frontend && npx vitest run src/components/chart/dashboard/__tests__/DashboardEditToolbar.test.tsx`

Expected: PASS, 4 tests.

- [ ] **Step 5: Wire it into the route**

In `frontend/src/routes/explore/DashboardView.tsx`, widen `addVirtualCard`'s parameter type
(line 288) and open the settings panel for a new iframe card, since — unlike text and heading —
it has no inline editor to focus:

```tsx
  function addVirtualCard(card_type: "text" | "heading" | "link" | "iframe", options: Record<string, unknown>) {
```

and replace the trailing focus line (line 301) with:

```tsx
    if (card_type === "text" || card_type === "heading") setNewCardId(id);
    // An iframe card has no inline editor, so the URL is entered in the settings panel. Open it
    // on add, the way addFilter does for a new filter.
    if (card_type === "iframe") openPanel({ kind: "card", cardId: id });
```

Add the toolbar prop at the call site (after line 515):

```tsx
              onAddIframe={() => addVirtualCard("iframe", { iframe: "" })}
```

No change is needed at line 292 (`sizeForViz(card_type)`) or line 341 (`vizKeyOf`) — both key on
the raw `card_type` string and pick up Task 3's entry automatically.

- [ ] **Step 6: Run the three gates**

Run each as its own command. Do not chain them, and do not pipe into `rtk` — a swallowed pipe
breaks the `&&` chain behind it and the later gates silently never run.

```bash
cd frontend && npm run lint
```

```bash
cd frontend && npx tsc --noEmit
```

```bash
cd frontend && npm run test
```

Expected: all green. Record the test count; it should be the previous total **plus exactly 53** —
31 in `iframe-url.test.ts`, 10 in `IFrameCard.test.tsx`, 4 in `DashboardEditToolbar.test.tsx`, and
1 + 2 + 5 appended to the three existing files. A count that differs by more than the tests you
added means a gate did not run.

- [ ] **Step 7: Commit**

```bash
git add frontend/src/components/chart/dashboard/DashboardEditToolbar.tsx \
        frontend/src/components/chart/dashboard/__tests__/DashboardEditToolbar.test.tsx \
        frontend/src/routes/explore/DashboardView.tsx
git commit -m "feat(dashboard): add an iframe card from the link toolbar dropdown"
```

---

## Task 6: Live check — measure, do not eyeball

**Files:** none. This task produces numbers for the PR body.

Every defect that mattered in the chart programme was found by measuring the running app. A
screenshot that "looks right" is not evidence, and a subagent's *simulation* of the browser is
not evidence either.

**Do not use the parity harness dashboard for this.** It carries 22 cards on byte-identical data
and adding a card to it breaks the comparison. Create a throwaway dashboard, then trash it.

- [ ] **Step 1: Build and deploy**

```bash
just deploy-kind
```

Expected: `deployment "egret" successfully rolled out`, SPA served at `http://localhost:30080`.

- [ ] **Step 2: Create a scratch dashboard and add an iframe card**

Log in at `http://localhost:30080` (admin@egret.local / Admin12345!), create a dashboard named
`ZZ Iframe Check` in the `demo` workspace, press Edit, open the link dropdown, choose **Iframe**.

Required: the settings panel opens by itself with an empty `iframe url or embed code` field, and
the new card occupies half the grid width and eight rows.

- [ ] **Step 3: Measure the allowed case**

Paste `https://www.loom.com/share/e5b8d3e07dc64e1f8b4a1a1a1a1a1a1a` into the field, press Save,
then evaluate in the page:

```js
(() => {
  const f = document.querySelector('.react-grid-item iframe');
  const card = f?.closest('.react-grid-item');
  const r = card?.getBoundingClientRect();
  return {
    count: document.querySelectorAll('.react-grid-item iframe').length,
    src: f?.getAttribute('src'),
    sandbox: f?.getAttribute('sandbox'),
    referrerpolicy: f?.getAttribute('referrerpolicy'),
    allow: f?.getAttribute('allow'),
    topNav: f?.sandbox.contains('allow-top-navigation'),
    modals: f?.sandbox.contains('allow-modals'),
    cardW: Math.round(r.width), cardH: Math.round(r.height),
    gridW: Math.round(document.querySelector('.react-grid-layout').getBoundingClientRect().width),
  };
})()
```

Required, exactly:

- `count: 1`
- `src: "https://www.loom.com/embed/e5b8d3e07dc64e1f8b4a1a1a1a1a1a1a"` — the share→embed rewrite
  fired; a `/share/` here means Task 1's `toEmbedUrl` is not on the path
- `sandbox: "allow-forms allow-popups allow-popups-to-escape-sandbox allow-presentation allow-same-origin allow-scripts"`
- `referrerpolicy: "strict-origin-when-cross-origin"`
- `allow: null`
- `topNav: false` and `modals: false` — read off the live `DOMTokenList`, not off the string
- `cardW` within 4px of `gridW / 2`, since 12 of 24 columns

- [ ] **Step 4: Measure the three refusals**

For each of `https://example.com`, `javascript://youtube.com/%0aalert(1)` and
`data://youtube.com/x`: put it in the field, press Save, reload the page, and evaluate:

```js
(() => {
  const card = document.querySelector('.react-grid-item');
  return {
    iframes: card.querySelectorAll('iframe').length,
    text: card.innerText.trim().replace(/\s+/g, ' '),
  };
})()
```

Required for all three: `iframes: 0`. This is the fail-closed assertion and it is the one the
whole workstream turns on — a stored value that fails the guard must never reach the DOM.

Required text, in **view** mode (reload without pressing Edit):
`"There was a problem rendering this content."` for all three, with **no hostname** in it.
Then press Edit and re-evaluate: `example.com` must now read
`"example.com can't be embedded in an iframe card. Only https pages on egret's allowed hosts can be embedded."`,
and the other two must still show the generic line.

- [ ] **Step 5: Prove the backend really is untouched**

The plan's central claim is that `card_type` is free text end to end. Prove it rather than
asserting it:

```bash
curl -s -H "Authorization: Bearer $TOKEN" \
  http://localhost:30080/api/ws/demo/dashboards/$SCRATCH_ID | jq '.cards[] | {card_type, size_x, size_y, options}'
```

Required: `card_type: "iframe"`, `size_x: 12`, `size_y: 8`, and `options.iframe` holding the
string verbatim. A `card_type: "chart"` here means something coerced it and the frontend-only
claim is wrong.

- [ ] **Step 6: Check the resize floor**

Back in edit mode, drag the card's resize handle as small as it will go, Save, and re-fetch.

Required: `size_x: 4, size_y: 3` — Task 3's minimum, not the `CARD_MIN_SIZE` 2×2 default.

- [ ] **Step 7: Clean up and record**

Move `ZZ Iframe Check` to the trash. Confirm the parity harness dashboard
(`/explore/demo/dashboards/8a196343-ad93-4968-8040-67598d29c3cb`) still reports 22 cards.

Open the PR with the Step 3 object quoted verbatim, the three `iframes: 0` results from Step 4,
the Step 5 `card_type` value, and the frontend test count. Then **stop** — the merge is the
user's call.

---

## Self-review checklist for the implementer

- [ ] No `<iframe>` element is ever constructed from a string that did not come back from
      `resolveIframeSrc` with `ok: true`. Grep the diff for `<iframe` and confirm there is
      exactly one, in `IFrameCard.tsx`.
- [ ] `IFrameCard.tsx` contains no URL parsing, no regex over a URL, and no host comparison —
      all of it lives in `iframe-url.ts` where it is tested without React.
- [ ] The `javascript:` and `data:` tests exist in **both** forms (bare colon and `://`), and
      Task 1 Step 5 mutation 1 confirmed that only the `://` pair fails when the scheme guard is
      removed.
- [ ] The `sandbox` assertion compares against the exported constant in the component test and
      against a token list in the policy test — not two copies of the same literal.
- [ ] `allow-top-navigation` appears nowhere in the diff except in the comment explaining why it
      is absent.
- [ ] `DashboardView.tsx` gained three lines and no logic: the widened parameter type, the
      `openPanel` call, and the toolbar prop.
- [ ] No test asserts a display string where the code keys on a raw value — the card-type
      branches key on `card_type`, and every test that exercises one asserts an effect
      (`iframes: 0`, a `src` attribute, an `onUpdateOptions` patch), not a heading.
- [ ] No action-button card was built. §5 of the design rules it out and this branch must not
      contain the word `action` as a card type.
