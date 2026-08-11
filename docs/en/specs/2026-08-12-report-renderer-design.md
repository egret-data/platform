# Report renderer — design

> Parent design: `docs/en/specs/2026-08-11-alerts-and-reports-design.md` (§5.3 fixes this
> subsystem's contract and security envelope; this document fills it in).
> Plan: `docs/en/plans/2026-08-11-alerts-and-reports-plan.md` (P4).

**Goal:** deliver a chart or dashboard on a schedule, as an image, a PDF, a CSV or plain text.

---

## 1. Why a headless browser at all

egret's charts are ECharts, drawn by a browser. The existing export path proves it:
`frontend/src/components/chart/export-chart.ts#downloadPng` reads the rendered `<svg>` out of the
DOM and rasterises it. There is no server-side path that produces the same picture.

The alternative — reimplementing the chart library in Rust so the backend can draw — was rejected.
It would mean two renderers that must agree pixel-for-pixel forever, and this repo's chart work
has already shown how much effort getting *one* renderer to match Metabase takes.

So: a real browser, headless, in a pod. That is the largest and riskiest item in the Alerts &
Reports programme, which is why it is last.

---

## 2. Decisions taken

- **All four formats in one phase.** `png`, `pdf`, `csv`, `text` ship together, even though only
  the first two need the renderer.
- **A render failure is an `error` run**, recorded with its reason and retried on the schedule's
  own cadence — the same vocabulary an engine refusal already uses. No bespoke retry machinery, no
  silent degradation to a text-only email.
- **The renderer is a required component.** The helm chart always installs it; there is one
  supported topology and no half-configured state to explain.

---

## 3. Architecture

```
scheduler tick ─► evaluator ─► render request ──────► egret-renderer (Deployment, ClusterIP)
                     │                                      │
                     │  POST /render {url, w, h, format}    │  Playwright + Chromium
                     │                                      ▼
                     │                            GET /embed/print/…?t=<token>
                     │                                      │
                     └──────────────── one-shot payload ◄───┘  GET /api/render/payload
                                                              (bearer token; single use,
                                                               ~60s, one object)
```

Four pieces, each testable alone:

| Piece | Where | Responsibility |
|---|---|---|
| Render token | `egret-automation` | Mint, redeem-once, expire |
| Payload endpoint | `egret-automation` | Everything the print page needs, in ONE response |
| Print route | SPA | Draw one object chrome-free; signal when it is done |
| `egret-renderer` | new image | Navigate, wait for the signal, capture, return bytes |

---

## 4. The print route, and the part that actually breaks

`/embed/print/:kind/:id?t=<token>` renders one chart or dashboard with no nav, no sidebar, no
padding — just the object on a white ground at the requested size.

**Knowing when to capture is the hard part, not capturing.** ECharts animates on mount, and web
fonts resolve after first paint. A renderer that waits a fixed two seconds will *usually* get a
finished chart and *occasionally* get one mid-animation, with bars part-height or labels in a
fallback face. That defect ships, because it is invisible to every test and reproduces perhaps one
run in twenty.

So readiness is explicit, not timed:

```js
// set by the print route, never by the renderer's guess
window.__EGRET_PRINT_READY__ = true
```

set only after **both**:

1. every chart on the page has reported ECharts' `finished` event, and
2. `document.fonts.ready` has resolved.

The renderer waits for that flag with a hard timeout. **A timeout is a failed render** — it returns
an error and the run is recorded `error`. It must never fall back to "capture whatever is on the
screen": a silently half-drawn chart in someone's inbox is worse than a missing one, because
nobody checks a report that arrived.

`prefers-reduced-motion` is forced on in the print context, so animations are short or absent —
belt and braces with the flag, not a replacement for it.

---

## 5. The token, and why the page fetches exactly once

§5.3 fixes the envelope: **single-object, single-use, ~60 seconds, bound to the schedule's owner.**
The renderer must never hold a credential that can read every dashboard in the platform — that is
the difference between a screenshot service and an exfiltration service.

Single-use forces a design decision. A normal dashboard page makes many authenticated calls (the
detail, then one per card). If the token authorised each of them it would not be single-use in any
meaningful sense, and "single-use" would be a comment rather than a property.

**So the print page makes exactly one request.** `GET /api/render/payload`, carrying the token in
an `Authorization: Bearer` header rather than the path — the navigation already had to put it in a
URL, but this call is ours to control, so it does not put it in a second one. It returns everything
the page needs — the object's spec *and* each card's already-executed data — assembled server-side through
the existing chart-run path, running as the owner. The page renders purely from that payload and
issues no further authenticated calls.

Consequences, all deliberate:

- Single-use is literal: the token row is **deleted** at redemption, inside the same transaction
  that reads it, so a replay finds nothing. Not a `used_at` flag that a second concurrent request
  could race past.
- The renderer's browser holds nothing reusable. Compromise the pod and you have one object, for
  under a minute, that the owner could already read.
- A print page cannot be talked into fetching something else — there is no second endpoint for it
  to reach.
- The cost is a larger response and a backend that assembles what the browser usually fetches
  piecemeal. Worth it.

Storage: `automation.render_tokens (id, token_hash, kind, object_id, owner_id, expires_at)`. The
token is a random opaque string; only its hash is stored, like every other bearer secret in this
repo. Expired rows are pruned by the same tick that prunes the run log.

**Authorization is the owner's, at render time** — the same rule as §5.2. If the owner has lost
access to the dashboard, minting fails and the run errors. A report must not keep delivering a
picture of data its owner may no longer open.

---

## 6. The renderer service

`POST /render` — cluster-only (ClusterIP, no Ingress):

```json
{ "url": "https://egret/embed/print/dashboard/<id>?t=<token>",
  "width": 1200, "height": 800, "format": "png", "scale": 2 }
```

Returns the bytes, or a non-2xx with a reason (`timeout`, `navigation failed`, `http <status>`).

- **One browser, a page per request**, with a concurrency cap. Chromium's memory is the binding
  constraint, not CPU; an unbounded page count is how this pod gets OOM-killed under a burst of
  schedules that all fire at the top of the hour.
- **Hard per-render timeout**, below the caller's, so egret sees a clean error rather than a
  hanging socket.
- **No credentials of its own.** The only secret it ever sees is the per-render token, in a URL it
  was handed.
- **PDF** is the same page through Chromium's print-to-PDF, so an image and a PDF of the same
  dashboard cannot disagree.
- `scale: 2` by default: a 1× screenshot of a chart looks soft in an email on any modern display.

Sizing comes from the schedule's `custom_width`/`custom_height`, defaulting to 1200×800 for a
dashboard and 800×600 for a chart.

---

## 7. Formats, and one constraint that needs a migration

`png` and `pdf` go through the renderer. `csv` and `text` never touch it — they are data the
backend already has, delivered through P3's email path.

**But a dashboard has no single table.** A CSV of "a dashboard" is either an arbitrary choice of
one card or a zip of many, and both are worse than saying so. Therefore:

> `csv` and `text` are valid only for a **chart** report. A **dashboard** report must be `png` or
> `pdf`.

That is a new CHECK constraint (P4 carries a migration) plus a validation rule that names the
field, following P1's pattern: the constraint is the guarantee, the validator is the message.

`text` for a chart report is its data rendered as a small plain-text table — the thing you can read
on a phone without opening an attachment.

---

## 8. Failure

| Situation | Result |
|---|---|
| Renderer unreachable / 5xx | `state=error`, reason recorded, retried next tick |
| Readiness timeout | `state=error` — never a partial capture |
| Owner lost access | `state=error` at token minting, before any render |
| Token expired or replayed | 404 at redemption; the render fails |
| Chart itself errors | The print page marks itself failed; readiness never sets; timeout |

One vocabulary, already in the run-log drawer, and `next_run_at` is recomputed on every path so a
failing report keeps trying rather than stopping dead.

---

## 9. Deployment

- New image `egret-renderer:dev`, built by `just images-build` and loaded by `just deploy-kind`,
  joining backend/frontend/kernel as the fourth.
- Helm always installs the Deployment + ClusterIP Service. Memory limit sized for Chromium (a
  request that is too small is an OOMKill under load, which reads as a flaky report).
- The backend reaches it by Service DNS, configured like the other seams.

---

## 10. How this gets verified

- **Unit/route**: token minted for the owner and nobody else; redeemed exactly once (a second
  attempt 404s); expiry honoured; the payload endpoint refuses an object the owner cannot read;
  the format/target constraint rejects a dashboard+csv at the API with a message naming the field.
- **Frontend**: the print route sets the readiness flag only after charts finish AND fonts resolve
  — asserted by withholding each in turn, since a flag that is set unconditionally is the bug.
- **Live check on kind** — the part that decides whether this works. A test cannot assert "the PNG
  contains the chart"; per the repo's own PNG-export experience the usable proxies are **pixel
  dimensions and byte count** (a 16×16 or 32×32 result means something grabbed a UI icon, not the
  chart). So: schedule a report against a real dashboard, let it fire, and open the delivered
  attachment.

---

## 11. What this design does not do

- **No print-styled dashboards.** One object per render, at a fixed size. Pagination, headers and
  footers are a later concern.
- **No caching.** Two schedules on the same dashboard render twice. Caching a picture of
  access-controlled data is a security design of its own.
- **No interactive filters at render time.** A report renders the dashboard's saved state.
- **No Slack/Teams unfurls.** Delivery stays email + webhook.

---

## 12. Risks

- **Chromium in a pod is heavy and fails in ways the rest of egret does not** — fonts missing from
  a slim base image is the classic one, and it degrades silently into boxes rather than erroring.
  The live check is what catches it; the readiness flag does not.
- **The one-shot payload can grow large** for a dashboard with many cards. Bounded by the same
  row limits the interactive path already applies.
- **A burst of schedules at the top of the hour** is the load shape this service will actually see,
  because people write `0 * * * *`. The concurrency cap plus the scheduler's existing batch limit
  are what keep that from becoming an outage.
- **The token travels in a URL, and URLs get logged.** This is unavoidable: the renderer navigates
  to a page, and a navigation carries its credential in the address. An ingress access log, a
  browser history file or a crash dump could therefore contain a live token. What makes it
  tolerable is precisely the envelope §5.3 fixed — under a minute, one use, one object, one
  owner — so a token recovered from a log is almost always already dead and, if not, grants a read
  the owner could perform anyway. It is an accepted risk, not an oversight; if it ever needs
  closing, the fix is a POST-and-redirect handshake so the credential never enters the address
  bar. The page's own payload call sends the token as a HEADER for the same reason, since that
  request is under our control.
